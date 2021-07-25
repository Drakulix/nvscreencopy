extern crate gl_generator;
extern crate wayland_scanner;

use wayland_scanner::{Side, generate_code};
use gl_generator::{Api, Fallbacks, Profile, Registry};
use std::{env, fs::File, path::PathBuf};

fn main() {
    let dest = PathBuf::from(&env::var("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed=build.rs");

    let mut file = File::create(&dest.join("egl_bindings.rs")).unwrap();
    Registry::new(
        Api::Egl,
        (1, 5),
        Profile::Core,
        Fallbacks::All,
        [
            "EGL_KHR_create_context",
            "EGL_EXT_create_context_robustness",
            "EGL_KHR_create_context_no_error",
            "EGL_MESA_platform_gbm",
            "EGL_WL_bind_wayland_display",
            "EGL_KHR_image_base",
            "EGL_EXT_image_dma_buf_import",
            "EGL_EXT_image_dma_buf_import_modifiers",
            "EGL_EXT_platform_base",
            "EGL_EXT_platform_device",
            "EGL_EXT_output_base",
            "EGL_EXT_output_drm",
            "EGL_EXT_device_drm",
            "EGL_EXT_device_enumeration",
            "EGL_EXT_device_query",
            "EGL_KHR_stream",
            "EGL_KHR_stream_producer_eglsurface",
            "EGL_EXT_stream_consumer_egloutput",
            "EGL_EXT_stream_acquire_mode",
            "EGL_KHR_stream_fifo",
            "EGL_NV_output_drm_flip_event",
            "EGL_NV_stream_attrib",
        ],
    )
    .write_bindings(gl_generator::GlobalGenerator, &mut file)
    .unwrap();

    // Location of the xml file, relative to the `Cargo.toml`
    let protocol_file = "resources/drm.xml";

    // Target directory for the generate files
    generate_code(
        protocol_file,
        &dest.join("drm.rs"),
        Side::Client, // Replace by `Side::Server` for server-side code
    );
}