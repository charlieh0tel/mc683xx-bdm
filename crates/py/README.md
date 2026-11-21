# mc683xx-bdm

Python bindings for the mc683xx BDM pod, a USB background debug mode
(BDM) pod for Motorola/Freescale CPU32 (683xx) microcontrollers built
on a Raspberry Pi Pico.  The module is `mc683xx_bdm`: one open pod
across calls, for scripts with loops and polling.

```python
import time
import mc683xx_bdm

with mc683xx_bdm.Pod("usb") as pod:  # or "usb:SERIAL", "unix:/tmp/pod.sock"
    print(pod.reset_with_breakpoint())  # breakpoint
    pod.write_register("PC", 0x100000)
    pod.write_memory(0x100000, bytes.fromhex("4e714afa"), size=2)
    pod.go()
    while pod.background is None:  # poll until it halts
        time.sleep(0.01)
    print(pod.registers())
    print(hex(pod.bus_read(0xFFFA00, 2)))  # SIMCR
```

`Pod.serve_gdb(scratch=...)` serves one gdb session over the open pod.
Types are in the package's stubs.  The pod, its firmware and the `bdm`
command line: https://github.com/charlieh0tel/mc683xx-bdm.
