# nvscreencopy

Although the situation has improved with the 470 driver on linux, nvidia gpus and wayland compositors on linux still remain in a poor state.

Especially if you are not using the larger compositors included in desktop-environments like GNOME or KDE.

A rather popular choice of a framework for smaller compositors has been [wlroots](https://github.com/swaywm/wlroots/),
which is not supporting proprietary drivers, and especially not the nvidia driver due to the only api supported being
the rather uncommon eglstream api.

Users of nvidia-gpus on desktop machines can instead rely on the [wlroots-eglstream](https://github.com/danvd/wlroots-eglstreams) fork of [@danvd](https://github.com/danvd).

Laptop users however often struggle as this port does not include support for multi-gpu setups, like commonly found in optimus laptops.

While you can run normal wlroots on the internal gpu be limiting the drm-devices used using the `WLR_DRM_DEVICES` environment variable (and special cmd arguments like the famous `--my-next-gpu-wont-be-nvidia` flag for [sway](https://github.com/swaywm/sway)), you cannot fully utilize your nvidia gpu this way.

You can use the gpu for rendering through e.g. [primus_vk](https://github.com/felixdoerre/primus_vk) (even [without relying on X11](https://github.com/felixdoerre/primus_vk/issues/24)) and you can of course use it's cuda capabilities, but notably you cannot utilize external monitors, if your
machine does not provide any ports, that are hooked up to your internal gpu.

An alternative for this use-case is using the nouveau driver, which is supported by wlroots-based compositors, but then you loose the performance and cuda capabilities of your card. Forcing you to switch drivers for either of these use-cases.

*nvscreencopy* aims to fix this by allowing you to clone outputs on your internal gpu to your nvidia gpu. On compositors supporting headless outputs (notable sway through its `create_output` ipc command), this even allows you to extend your workspace.

# How does this work

*nvscreencopy* is basically `primus_vk` but backwards.

1. It receives an output image through the `export-dmabuf` protocol.
2. (tries to import the dmabuf into the nvidia driver, but even with 470 this still fails)
3. falls back to doing a cpu copy, by
  1. reading out the image into main memory
  2. uploading the image to the nvidia gpu
4. rendering the image via the eglstream protocol

Because nvscreencopy is the only process requesting kms capabilities of the nvidia gpu this works without any additional permission.

# How do I use this

```
$ ./nvscreencopy --help

nvscreencopy 0.1
Drakulix <nvscreencopy@drakulix.de>
Implements screen mirroring to nvidia gpus using the wayland export-dmabuf protocol

USAGE:
    nvscreencopy [OPTIONS] [SUBCOMMAND]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -c, --connector <NAME>    Connector to clone onto. By default takes the first connected one it finds
    -m, --mode <MODE>         Sets the outputs mode, by default it mirrors the mode of the source. Use this if they are
                              incompatible, the result will be streched. Format "WIDTHxHEIGHT"
    -s, --source <SRC>        Sets the monitor to copy from, checks by comparing the monitor make to contain the given
                              value. Default is "headless".

SUBCOMMANDS:
    help               Prints this message or the help of the given subcommand(s)
    list-connectors    lists available sources
    list-sources       lists available sources
```

# How do I build this

nvscreencopy is written in Rust and uses [smithay](https://github.com/Smithay/smithay) - which is a compositor framework on its own - to facilitate the copy.

That means for building you need [rustc](https://www.rust-lang.org/tools/install) installed.

You will also need development packages of the following libraries for building:
- libudev
- libwayland

Then run `cargo build --release` in this directory.

# Known limitations

- nvscreencopy currently only supports one source and one destination. KMS permissions will likely interfere with running nvscreencopy multiple times for different outputs, therefor support for multiple copies running in parallel needs to be added the nvscreencopy directly.
- nvscreencopy could likely do better on performance, the cpu copy is rather slow and is not suited for low-latency applications.
  - But to do try that, we would need to control memory placement of the buffers, which either requires changing the compositor (which nvscreencopy explicitly avoids) or having a more powerful api then EGL for this purpose. Vulkan could likely be used, but smithay is currently lacking a vulkan renderer.
- This only works on compositors implementing the wlr-export-dmabuf protocol. wlr-screencopy could be supported as an alternative in the future.

# Can this also be used to proxy applications?

- No this is specially not what nvscreencopy does, use primus_vk or bumblebee/primus for that purpose.
- (Technically this could be done by creating a proxying wayland-server, receiving EGLImages from the clients eglstreams, exporting them as dmabuf and try to attach those to the surfaces instead, but that would be a far larger project. Instead I focus my time on [my own compositor](https://github.com/Drakulix/fireplace), which will have some kind of nvidia support out-of-the-box, where this is way easier to implement.)

# I have issues / this does not run very well

- Feel free to [open an issue](https://github.com/Drakulix/nvscreencopy/issues/new/choose), I might have an idea and find the time to fix this
- BUT this is mostly a weekend hack.
  - I hope this will continue to work long enough for me to work on my compositor.
  - As long as this thing is usable for me, I have no further interest to spent much time on this.
  - I am sharing this with the best intentions, but be warned, this is not very reliable.
 