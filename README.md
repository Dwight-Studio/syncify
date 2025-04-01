# Syncify
A peer-to-peer, cross-platform file synchronization client

## Compilation
### Windows

Before compiling on windows, you need to install rust (using [rustup](https://www.rust-lang.org/fr/learn/get-started)) and [MySYS2](https://www.msys2.org/).

Using MSYS2, you must install the following dependencies:
- mingw-w64-ucrt-x86_64-pkg-config
- mingw-w64-ucrt-x86_64-gtk4
- mingw-w64-ucrt-x86_64-glibc2
- mingw-w64-ucrt-x86_64-libadwaita

Remember to add **C:\msys2\ucrt64\bin** to the PATH environment variable.

### Linux

Before compiling on linux, you need to install rust (using [rustup](https://www.rust-lang.org/fr/learn/get-started)).

On Fedora, here is the packages you must install:
- gtk4-devel
- libadwaita-devel
- gcc

### MacOS

Before compiling on macos, you need to install rust (using [rustup](https://www.rust-lang.org/fr/learn/get-started)) and
[MacPorts](https://ports.macports.org/).

Using MacPorts, you must install the following dependencies:
- pkgconfig
- glib2
- gtk4-devel
- libadwaita
