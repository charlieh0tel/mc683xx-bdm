# mc683xx-bdm

The host side of the mc683xx BDM pod, a USB background debug mode (BDM)
pod for Motorola/Freescale CPU32 (683xx) microcontrollers, primarily
the MC68331, built on a Raspberry Pi Pico.

- `bdm`, the command line: halt, reset and run the target, read and
  write its registers and memory, and serve gdb's remote protocol
  (`bdm gdb`).
- The library: the pod's USB requests as calls (`pod`), the target
  behind it as `mc683xx_target::Cpu32Target` (`target`), registers by
  name, a gdb session over it (`gdb`), and a stand-in pod on a stream
  socket (`stand_in`).

```
cargo install mc683xx-bdm
bdm list                       # each pod on USB: serial number, bus-ports
bdm reset                      # into background mode
bdm regs
bdm dump 0xfffa00 64
bdm gdb --scratch 0x100000     # gdb on 127.0.0.1:1234
```

On Linux without root, the pod needs the udev rule
`packaging/udev/70-mc683xx-pod.rules` from the repository, or the
Debian package, which carries it.  The pod's firmware and `bdm` must
speak the same protocol (`bdm version`).

The pod, its firmware, the Debian package, the Python bindings
(`pip install mc683xx-bdm`) and the carrier board:
https://github.com/charlieh0tel/mc683xx-bdm.

License: GPL-3.0-only.
