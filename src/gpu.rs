use anyhow::{Context, Result};
use smithay::{
    backend::{
        allocator::{Fourcc},
        drm::{DrmDevice, DrmSurface},
        egl::{
            context::{GlAttributes, PixelFormatRequirements},
            EGLContext, EGLDisplay, EGLSurface,
        },
        renderer::gles2::Gles2Renderer,
        udev::{driver, UdevBackend},
    },
    reexports::drm::{
        control::{
            connector::{Info as ConnectorInfo, Interface, State as ConnectorState},
            dumbbuffer::DumbBuffer,
            framebuffer, Device as ControlDevice,
        },
        Device as DrmDeviceNode,
    },
};

use crate::egl::{EGLDeviceEXT, EglStreamSurface};

use std::{
    fs::File,
    os::unix::io::{AsRawFd, RawFd},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

pub struct TargetGPU {
    pub renderer: Gles2Renderer,
    pub surface: Rc<EGLSurface>,
    _display: EGLDisplay,
    _device: EGLDeviceEXT,
    _drm_surface: DrmSurface<Fd>,
    _fb: framebuffer::Handle,
    _db: DumbBuffer,
}

impl Drop for TargetGPU {
    fn drop(&mut self) {
        let _ = self._drm_surface.destroy_framebuffer(self._fb);
        let _ = self._drm_surface.destroy_dumb_buffer(self._db);
    }
}

pub struct RenderGPU {
    pub renderer: Gles2Renderer,
    _display: EGLDisplay,
    _device: EGLDeviceEXT,
}

pub struct Fd {
    fd: File,
}

impl Fd {
    pub fn open<P: AsRef<Path>>(file: &P) -> std::io::Result<Fd> {
        Ok(Fd::new(File::open(file.as_ref())?))
    }

    pub fn new(file: File) -> Fd {
        Fd {
            fd: file,
        }
    }
}

impl Clone for Fd {
    fn clone(&self) -> Fd {
        Fd {
            fd: self
                .fd
                .try_clone()
                .expect("Failed to clone file descriptor"),
        }
    }
}

impl AsRawFd for Fd {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}
impl DrmDeviceNode for Fd {}

pub fn find_nvidia_gpu(log: slog::Logger) -> Option<PathBuf> {
    let seat = std::env::var("XDG_SEAT").expect("XDG_SEAT is not set");
    let udev_backend = UdevBackend::new(seat, log).ok()?;

    // Enumerate gpus
    let path = udev_backend
        .device_list()
        .flat_map(|(dev, path)| driver(dev).ok().and_then(|x| x.map(|x| (x, path))))
        .flat_map(|(driver_os, path)| driver_os.into_string().ok().map(|x| (x, path)))
        .filter(|(driver, _)| driver.contains("nvidia"))
        .map(|(_, path)| path.to_path_buf())
        .next();

    path
}

pub fn init_render_gpu(fd: Fd, log: slog::Logger) -> Result<RenderGPU> {
    let egl_device = EGLDeviceEXT::new(fd, log.clone())?;
    let display = EGLDisplay::new(&egl_device, log.clone())?;
    let context = EGLContext::new(&display, log.clone())?;
    let renderer = unsafe { Gles2Renderer::new(context, log.clone())? };

    Ok(RenderGPU {
        _device: egl_device,
        _display: display,
        renderer,
    })
}

pub fn init_target_gpu(
    path: PathBuf,
    connector: Option<&str>,
    mode: (i32, i32),
    log: slog::Logger,
) -> Result<(TargetGPU, DrmDevice<Fd>)> {
    let fd = Fd {
        fd: File::open(&path)?,
    };
    let device = DrmDevice::new(fd.clone(), false, log.clone())?;
    let egl_device = EGLDeviceEXT::new(fd, log.clone())?;
    // Get a set of all modesetting resource handles (excluding planes):
    let res_handles = device.resource_handles().unwrap();

    // Use first connected connector
    let connector_info: ConnectorInfo = res_handles
        .connectors()
        .iter()
        .map(|conn| device.get_connector(*conn).unwrap())
        .filter(|conn| conn.state() == ConnectorState::Connected)
        .inspect(|conn| {
            slog::info!(
                log,
                "Connected: {:?}-{:?}",
                conn.interface(),
                conn.interface_id()
            )
        })
        .find(|conn| {
            if let Some(connector) = connector {
                format!(
                    "{}-{}",
                    match conn.interface() {
                        Interface::VGA => "VGA",
                        Interface::DVII | Interface::DVID | Interface::DVIA => "DVI",
                        Interface::LVDS => "LVDS",
                        Interface::DisplayPort => "DP",
                        Interface::HDMIA | Interface::HDMIB => "HDMI",
                        Interface::EmbeddedDisplayPort => "eDP",
                        _ => "Unsupported",
                    },
                    conn.interface_id()
                ) == connector
            } else {
                true
            }
        })
        .with_context(|| "Unable to find connector")?;

    let crtc = connector_info
        .encoders()
        .iter()
        .filter_map(|e| *e)
        .flat_map(|encoder_handle| device.get_encoder(encoder_handle))
        .flat_map(|encoder_info| res_handles.filter_crtcs(encoder_info.possible_crtcs()))
        .next()
        .with_context(|| "Unable to find suitable crtc")?;

    let drm_mode = connector_info
        .modes()
        .iter()
        .find(|drm_mode| drm_mode.size() == (mode.0 as u16, mode.1 as u16))
        .cloned()
        .expect("Output mode not supported by connector");
    let db = device.create_dumb_buffer((mode.0 as u32, mode.1 as u32), Fourcc::Argb8888, 32)?;
    let fb = device.add_framebuffer(&db, 24, 32)?;
    let drm_surface = device.create_surface(crtc, drm_mode, &[connector_info.handle()])?;
    let plane = drm_surface.plane();
    drm_surface.commit([&(fb, plane)].iter().cloned(), true)?;
    std::thread::sleep(Duration::from_secs(1));

    let egl_display = EGLDisplay::new(&egl_device, log.clone())?;
    let egl_context = EGLContext::new_with_config(
        &egl_display,
        GlAttributes {
            version: (3, 0),
            profile: None,
            debug: cfg!(debug_assertions),
            vsync: false,
        },
        PixelFormatRequirements {
            hardware_accelerated: Some(true),
            color_bits: Some(3),
            alpha_bits: Some(0),
            depth_bits: Some(1),
            ..Default::default()
        },
        log.clone(),
    )?;
    let surface = EglStreamSurface::new(crtc, plane, mode, log.clone());
    let egl_surface = Rc::new(EGLSurface::new(
        &egl_display,
        egl_context.pixel_format().unwrap(),
        egl_context.config_id(),
        surface,
        log.clone(),
    )?);
    let renderer = unsafe { Gles2Renderer::new(egl_context, log.clone())? };

    Ok((
        TargetGPU {
            _device: egl_device,
            _display: egl_display,
            surface: egl_surface,
            renderer,
            _drm_surface: drm_surface,
            _fb: fb,
            _db: db,
        },
        device,
    ))
}
