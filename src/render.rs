use anyhow::Result;
use smithay::backend::{allocator::{dmabuf::Dmabuf, Buffer}, egl::{EGLError, SwapBuffersError}, renderer::{
        gles2::{Gles2Error, Gles2Renderer, Gles2Texture},
        Bind, Frame, ImportDma, Renderer, Transform, Unbind,
    }};

use crate::{CopyState, WaylandState};

pub fn create_texture(
    renderer: &mut Gles2Renderer,
    width: i32,
    height: i32,
) -> Result<Gles2Texture, Gles2Error> {
    renderer.with_context(|renderer, gl| unsafe {
        let mut tex = 0;
        gl.GenTextures(1, &mut tex);
        Gles2Texture::from_raw(renderer, tex, (width, height).into())
    })
}

fn import_bitmap(
    renderer: &mut Gles2Renderer,
    texture: &mut Gles2Texture,
    image: &[u8],
    width: i32,
    height: i32,
) -> Result<(), Gles2Error> {
    use smithay::backend::renderer::gles2::ffi;

    renderer.with_context(|_renderer, gl| unsafe {
        let tex = texture.tex_id();
        gl.BindTexture(ffi::TEXTURE_2D, tex);
        gl.TexParameteri(
            ffi::TEXTURE_2D,
            ffi::TEXTURE_WRAP_S,
            ffi::CLAMP_TO_EDGE as i32,
        );
        gl.TexParameteri(
            ffi::TEXTURE_2D,
            ffi::TEXTURE_WRAP_T,
            ffi::CLAMP_TO_EDGE as i32,
        );
        gl.TexImage2D(
            ffi::TEXTURE_2D,
            0,
            ffi::RGBA as i32,
            width,
            height,
            0,
            ffi::RGBA,
            ffi::UNSIGNED_BYTE as u32,
            image.as_ptr() as *const _,
        );
        gl.BindTexture(ffi::TEXTURE_2D, 0);
    })
}

fn copy_by_import(state: &mut WaylandState, buf: &Dmabuf) -> Result<()> {
    // that this works is actually very very unlikely.
    //
    // the src buffer is likely in a tiled layout incompatible with nvidia
    // and also not necessarily in memory accessible by the nvidia gpu.
    //
    // We could try to do a better copy with a vulkan renderer (essentially doing
    // what primus_vk is doing but in reverse), but smithay currently has no
    // vulkan renderer and I do not want to deal with that now.
    //
    // So we just fall back to a cpu copy in most (if not all) cases.
    let imported = state.target.renderer.import_dmabuf(buf)?;
    state.texture = imported;
    Ok(())
}

fn copy_by_cpu(state: &mut WaylandState, buf: &Dmabuf) -> Result<()> {
    let (w, h) = buf.size().into();
    state.render.renderer.bind(buf.clone())?;

    let buffer_ptr = state.buffer.as_mut_ptr() as *mut _;
    state.render.renderer.with_context(|_renderer, gl| unsafe {
        use smithay::backend::renderer::gles2::ffi;
        gl.ReadPixels(0, 0, w, h, ffi::RGBA, ffi::UNSIGNED_BYTE, buffer_ptr);
    })?;
    state.render.renderer.unbind()?;
    import_bitmap(
        &mut state.target.renderer,
        &mut state.texture,
        &state.buffer,
        w,
        h,
    )?;
    Ok(())
}

pub fn render_dmabuf(state: &mut WaylandState, buf: Dmabuf) -> Result<()> {
    match state.copy {
        None => {
            if copy_by_import(state, &buf).is_ok() {
                slog::info!(state.log, "Copy path: DirectImport");
                state.copy = Some(CopyState::DirectImport);
            } else if copy_by_cpu(state, &buf).is_ok() {
                slog::info!(state.log, "Copy path: CPUCopy");
                state.copy = Some(CopyState::CPUCopy);
            } else {
                panic!("Could not determine working copy path");
            }
        }
        Some(CopyState::DirectImport) => copy_by_import(state, &buf)?,
        Some(CopyState::CPUCopy) => copy_by_cpu(state, &buf)?,
    };

    state
        .target
        .renderer
        .bind(state.target.surface.clone())
        .expect("Failed to bind surface");
    let texture = &state.texture;
    state
        .target
        .renderer
        .render(
            state.dest_size,
            Transform::Normal,
            |_, frame| {
                frame.render_texture_at(texture, (0.0, 0.0).into(), 1, 1.0, Transform::Normal, 1.0)
            },
        )??;
    match state.target.surface.swap_buffers() {
        Err(SwapBuffersError::EGLSwapBuffers(x @ EGLError::Unknown(0x3353)))
        | Err(SwapBuffersError::EGLSwapBuffers(x @ EGLError::Unknown(0x321c)))
        | Err(SwapBuffersError::EGLSwapBuffers(x @ EGLError::BadSurface)) => {
            slog::warn!(state.log, "Temporary Error: {:?}", x);
            state
                .try_again
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Err(err) => panic!("Swapping buffers failed: {}", err),
        _ => {}
    };

    Ok(())
}
