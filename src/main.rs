use anyhow::Context;
use calloop::{generic::Generic, Dispatcher, EventLoop, Interest, PostAction};
use clap::{App, Arg, SubCommand};
use sctk::environment::Environment;
use slog::{o, Drain};
use smithay::{
    backend::{
        allocator::{
            dmabuf::{Dmabuf, DmabufBuilder, DmabufFlags},
            Fourcc, Modifier,
        },
        drm::{DrmDevice, DrmEvent},
        renderer::gles2::Gles2Texture,
    },
    reexports::drm::control::{
        connector::{Interface, State as ConnectorState},
        Device,
    },
    utils::{Physical, Size},
};
use smithay_client_toolkit::{
    self as sctk,
    reexports::client::{protocol::wl_output, Display},
    reexports::protocols::wlr::unstable::export_dmabuf::v1::client::{
        zwlr_export_dmabuf_frame_v1::{self as export_dmabuf_frame, Event as ExportDmabufEvent},
        zwlr_export_dmabuf_manager_v1::ZwlrExportDmabufManagerV1 as ExportDmabufManager,
    },
};
use wayland_client::{DispatchData, EventQueue, Main};

use std::{
    convert::TryFrom,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

mod drm;
mod egl;
mod gpu;
mod render;
use self::drm::{wl_drm, WlDrmHandler};

struct Env {
    outputs: sctk::output::OutputHandler,
    export_dmabuf: sctk::environment::SimpleGlobal<ExportDmabufManager>,
    drm: WlDrmHandler,
}

sctk::environment!(Env,
    singles = [
        ExportDmabufManager => export_dmabuf,
        wl_drm::WlDrm => drm,
    ],
    multis = [
        wl_output::WlOutput => outputs,
    ]
);

enum CopyState {
    DirectImport,
    CPUCopy,
}

pub struct WaylandState {
    target: gpu::TargetGPU,
    render: gpu::RenderGPU,
    dmabuf: Option<(DmabufBuilder, u64)>,
    try_again: AtomicBool,
    dest_size: Size<i32, Physical>,
    buffer: Vec<u8>,
    texture: Gles2Texture,
    copy: Option<CopyState>,
    log: slog::Logger,
}

struct CalloopState {
    wayland_state: WaylandState,
    output: wl_output::WlOutput,
    event_queue: EventQueue,
    environment: Environment<Env>,
}

pub fn handle_frame(
    frame: Main<export_dmabuf_frame::ZwlrExportDmabufFrameV1>,
    event: ExportDmabufEvent,
    mut data: DispatchData,
) {
    let mut state: &mut WaylandState = data.get().unwrap();
    match event {
        ExportDmabufEvent::Frame {
            width,
            height,
            buffer_flags,
            format,
            mod_high,
            mod_low,
            ..
        } => {
            state.dmabuf = Some((
                Dmabuf::builder(
                    (width as i32, height as i32),
                    Fourcc::try_from(format).unwrap(),
                    DmabufFlags::from_bits_truncate(buffer_flags),
                ),
                (((mod_high as u64) << 32) | mod_low as u64),
            ));
        }
        ExportDmabufEvent::Object {
            fd,
            offset,
            stride,
            plane_index,
            ..
        } => {
            let (dmabuf, modifier) = state
                .dmabuf
                .as_mut()
                .expect("Object event before Frame event");
            dmabuf.add_plane(fd, plane_index, offset, stride, Modifier::from(*modifier));
        }
        ExportDmabufEvent::Ready { .. } => {
            slog::debug!(state.log, "Frame ready");
            let (dmabuf, _) = state
                .dmabuf
                .take()
                .expect("Object event before Frame event");
            let buf = dmabuf.build().expect("Failed to build dmabuf");
            slog::debug!(state.log, "Original Dmabuf: {:?}", buf);
            render::render_dmabuf(state, buf).expect("Failed to render");
            frame.destroy();
        }
        ExportDmabufEvent::Cancel {
            reason: export_dmabuf_frame::CancelReason::Permanent,
        } => panic!("Output died"),
        ExportDmabufEvent::Cancel { .. } => {
            slog::debug!(state.log, "Frame cancelled");
            frame.destroy();
            state
                .try_again
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        _ => panic!("Unknown export-dmabuf event"),
    }
}

fn main() -> anyhow::Result<()> {
    let matches = App::new("nvscreencopy")
        .version("0.2")
        .author("Drakulix <nvscreencopy@drakulix.de>")
        .about("Implements screen mirroring to nvidia gpus using the wayland export-dmabuf protocol")
        .arg(Arg::with_name("DEST")
            .short("c")
            .long("connector")
            .value_name("NAME")
            .help("Connector to clone onto. By default takes the first connected one it finds")
            .takes_value(true))
        .arg(Arg::with_name("SRC")
            .short("s")
            .long("source")
            .help("Sets the monitor to copy from, checks by comparing the monitor make to contain the given value. Default is \"headless\".")
            .takes_value(true))
        .arg(Arg::with_name("MODE")
            .short("m")
            .long("mode")
            .help("Sets the outputs mode, by default it mirrors the mode of the source. Use this if they are incompatible, the result will be streched. Format \"WIDTHxHEIGHT\"")
            .validator(|input| {
                let parts = input
                    .split("x")
                    .map(|x| u32::from_str_radix(x, 10))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|err| format!("Failed to parse numeric values of mode: {}", err))?;
                if parts.len() != 2 {
                    return Err(String::from("Mode with less/more then two values"));
                }
                Ok(())
            })
            .takes_value(true)
        )
        .subcommand(SubCommand::with_name("list-sources")
                    .about("lists available sources"))
        .subcommand(SubCommand::with_name("list-connectors")
                    .about("lists available sources"))
        .get_matches();

    
    // A logger facility, here we use the terminal here
    let log = if matches.subcommand().1.is_some() {
        slog::Logger::root(slog::Discard.fuse(), o!())
    } else {
        slog::Logger::root(slog_async::Async::default(slog_term::term_full().fuse()).fuse(), o!())
    };
    let _guard = slog_scope::set_global_logger(log.clone());
    slog_stdlog::init().expect("Could not setup log backend");

    let connector = matches.value_of("DEST");
    let monitor = matches.value_of("SRC").unwrap_or("headless");
    let dest_mode = matches.value_of("MODE").map(|x| {
        let parts = x
            .split("x")
            .map(|x| u32::from_str_radix(x, 10))
            .map(|x| x.map(|x| x as i32))
            .collect::<Result<Vec<i32>, _>>()
            .unwrap(); //already validated
        (parts[0], parts[1])
    });

    // Connect to the wayland server
    let client_display = Display::connect_to_env().unwrap();
    let mut event_loop: EventLoop<'_, CalloopState> = EventLoop::try_new().unwrap();
    let mut event_queue = client_display.create_event_queue();
    let attached_display = client_display.attach(event_queue.token());
    let environment = sctk::environment::Environment::new(
        &attached_display,
        &mut event_queue,
        Env {
            outputs: sctk::output::OutputHandler::new(),
            export_dmabuf: sctk::environment::SimpleGlobal::new(),
            drm: WlDrmHandler::new(),
        },
    )?;

    // get the requested output
    let mut output = None;
    let outputs = environment.get_all_outputs();

    if matches.subcommand_matches("list-sources").is_some() {
        for output in outputs {
            sctk::output::with_output_info(&output, |info| {
                println!("{}", info.make);
            });
        }
        return Ok(());
    }

    for test_output in outputs {
        if let Some(Some(mode)) = sctk::output::with_output_info(&test_output, |info| {
            if info.make.contains(monitor) {
                for mode in &info.modes {
                    if mode.is_current {
                        return Some(mode.clone());
                    }
                }
            }
            None
        }) {
            output = Some((test_output, mode));
        }
    }
    let (output, mode) = output.with_context(|| "Unable to find headless output")?;

    // init target gpu
    let path = gpu::find_nvidia_gpu(log.clone())
        .with_context(|| "Failed to automatically detect nvidia gpu")?;
    if matches.subcommand_matches("list-connectors").is_some() {
        let fd = gpu::Fd::open(&path)?;
        let device = DrmDevice::new(fd, false, log)?;
        let res_handles = device.resource_handles().unwrap();
        for conn in res_handles
            .connectors()
            .iter()
            .map(|conn| device.get_connector(*conn).unwrap())
        {
            println!(
                "{}-{}: {}",
                match conn.interface() {
                    Interface::VGA => "VGA",
                    Interface::DVII | Interface::DVID | Interface::DVIA => "DVI",
                    Interface::LVDS => "LVDS",
                    Interface::DisplayPort => "DP",
                    Interface::HDMIA | Interface::HDMIB => "HDMI",
                    Interface::EmbeddedDisplayPort => "eDP",
                    _ => "Unsupported",
                },
                conn.interface_id(),
                match conn.state() {
                    ConnectorState::Connected => "Connected",
                    ConnectorState::Disconnected => "Disconnected",
                    _ => "Unknown",
                }
            )
        }
        return Ok(());
    }
    slog::info!(log, "Found nvidia gpu {}", path.display());
    let (mut target_gpu, target_event_source) = gpu::init_target_gpu(
        path,
        connector,
        dest_mode.unwrap_or(mode.dimensions),
        log.clone(),
    )?;

    // init render gpu
    let path = PathBuf::from(environment.with_inner(|env| env.drm.path()));
    slog::info!(log, "Found wl gpu {}", path.display());
    let fd = gpu::Fd::open(&path)?;
    event_queue.sync_roundtrip(&mut (), |_, _, _| ())?;
    let render_gpu = gpu::init_render_gpu(fd, log.clone())?;

    let conn_fd = client_display.get_connection_fd();
    let _wayland_token = event_loop
        .handle()
        .insert_source(
            Generic::from_fd(conn_fd, Interest::READ, calloop::Mode::Level),
            move |_, _, state: &mut CalloopState| {
                slog::debug!(state.wayland_state.log, "Wayland event");
                match state
                    .event_queue
                    .dispatch(&mut state.wayland_state, |event, object, _| {
                        panic!(
                            "[calloop] Encountered an orphan event: {}@{} : {}",
                            event.interface,
                            object.as_ref().id(),
                            event.name
                        );
                    }) {
                    Ok(_) => Ok(PostAction::Continue),
                    Err(e) => {
                        panic!("I/O error on the Wayland display: {}", e)
                    }
                }
            },
        )
        .expect("Failed to add display to event loop");

    let texture = render::create_texture(
        &mut target_gpu.renderer,
        mode.dimensions.0,
        mode.dimensions.1,
    )
    .unwrap();
    let wl_state = WaylandState {
        render: render_gpu,
        target: target_gpu,
        dmabuf: None,
        log: log.clone(),
        buffer: vec![0u8; (mode.dimensions.0 * mode.dimensions.1 * 4) as usize],
        texture,
        copy: None,
        dest_size: dest_mode
            .map(|(w, h)| Size::from((w as i32, h as i32)))
            .unwrap_or(Size::from((mode.dimensions.0, mode.dimensions.1))),
        try_again: AtomicBool::new(false),
    };

    let event_dispatcher = Dispatcher::new(
        target_event_source,
        move |event, _, state: &mut CalloopState| match event {
            DrmEvent::VBlank(_crtc) => {
                let manager = state
                    .environment
                    .get_global::<ExportDmabufManager>()
                    .expect("No Export-DMABUF protocol");
                let frame = manager.capture_output(1, &state.output);
                frame.quick_assign(handle_frame);
            }
            DrmEvent::Error(error) => slog::error!(log, "{:?}", error),
        },
    );
    let _nv_token = event_loop
        .handle()
        .register_dispatcher(event_dispatcher.clone())
        .unwrap();

    let mut state = CalloopState {
        wayland_state: wl_state,
        environment,
        output,
        event_queue,
    };

    event_loop
        .run(Duration::from_secs(1), &mut state, |state| {
            if state.wayland_state.try_again.swap(false, Ordering::SeqCst) {
                let manager = state
                    .environment
                    .get_global::<ExportDmabufManager>()
                    .expect("No Export-DMABUF protocol");
                let frame = manager.capture_output(1, &state.output);
                slog::debug!(state.wayland_state.log, "Init frame");
                frame.quick_assign(handle_frame);
            }
            state
                .event_queue
                .sync_roundtrip(&mut state.wayland_state, |event, object, _| {
                    panic!(
                        "[calloop] Encountered an orphan event: {}@{} : {}",
                        event.interface,
                        object.as_ref().id(),
                        event.name
                    );
                })
                .expect("Wayland display died");
        })
        .map_err(|x| x.into())
}
