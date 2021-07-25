// Re-export only the actual code, and then only use this re-export
// The `generated` module below is just some boilerplate to properly isolate stuff
// and avoid exposing internal details.
//
// You can use all the types from my_protocol as if they went from `wayland_client::protocol`.
pub use generated::client::wl_drm;

mod generated {
    // The generated code tends to trigger a lot of warnings
    // so we isolate it into a very permissive module
    #![allow(dead_code,non_camel_case_types,unused_unsafe,unused_variables)]
    #![allow(non_upper_case_globals,non_snake_case,unused_imports)]

    pub mod client {
        // These imports are used by the generated code
        pub(crate) use wayland_client::{Main, Attached, Proxy, ProxyMap, AnonymousObject};
        pub(crate) use wayland_commons::map::{Object, ObjectMetadata};
        pub(crate) use wayland_commons::{Interface, MessageGroup};
        pub(crate) use wayland_commons::wire::{Argument, MessageDesc, ArgumentType, Message};
        pub(crate) use wayland_commons::smallvec;
        pub(crate) use wayland_client::sys;
        pub(crate) use wayland_client::protocol::wl_buffer;
        include!(concat!(env!("OUT_DIR"), "/drm.rs"));
    }
}

use std::{
    cell::RefCell,
    rc::Rc,
};

use wayland_client::{Attached, DispatchData, protocol::wl_registry};

pub struct WlDrmHandler {
    global: Option<Attached<wl_drm::WlDrm>>,
    path: Rc<RefCell<Option<String>>>,
}

impl WlDrmHandler {
    pub fn new() -> WlDrmHandler {
        WlDrmHandler { global: None, path: Rc::new(RefCell::new(None)) }
    }

    pub fn path(&self) -> String {
        self.path.borrow().clone().expect("WlDrm was not advertised")
    }
}

impl smithay_client_toolkit::environment::GlobalHandler<wl_drm::WlDrm> for WlDrmHandler {
    fn created(
        &mut self,
        registry: Attached<wl_registry::WlRegistry>,
        id: u32,
        _version: u32,
        _: DispatchData,
    ) {
        let wl_drm = registry.bind::<wl_drm::WlDrm>(1, id);
        let path_store = self.path.clone();
        wl_drm.quick_assign(move |_, event, _| {
            match event {
                wl_drm::Event::Device { name } => {
                    *path_store.borrow_mut() = Some(name);
                },
                wl_drm::Event::Authenticated => {
                    println!("AUTHENTICATED");
                }
                _ => {},
            }
        });
        self.global = Some((*wl_drm).clone());
    }
    
    fn get(&self) -> Option<Attached<wl_drm::WlDrm>> {
        self.global.clone()
    }
}