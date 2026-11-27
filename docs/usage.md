# Using the pod

## Access without root (Linux)

The pod needs the udev rule `packaging/udev/70-mc683xx-pod.rules`. The
Debian package installs it; otherwise copy it into
`/etc/udev/rules.d/` and replug the pod.

The rule gives the pod, and the Pico's USB bootloader (2e8a:0003,
2e8a:000f), to the user at the seat (`TAG+="uaccess"`) and to group
`plugdev`, mode 0660. Uaccess does not reach a login over
ssh, so add a remote user to the group: `sudo adduser USER plugdev`.
1209:0001 is pid.codes' shared test pair, so the rule also matches
anything else that uses it.

## Firmware

`bdm bootsel` reboots the pod into the Pico's bootloader; copy
`pod-fw-rp2040.uf2` onto the RPI-RP2 drive it mounts, or, where
nothing mounts it (a login over ssh), `picotool load -x
pod-fw-rp2040.uf2`. The UF2 is in
each GitHub Release and, with the Debian package, at
`/usr/share/mc683xx-bdm/`. A blank Pico starts in the bootloader. The
release UF2 is for the Pico; a Pico 2 needs its own build
([development.md](development.md)).

## `bdm`

```
bdm pins                      # FREEZE, RESET, DS, BERR, target VDD
bdm reset                     # reset into background mode
bdm regs                      # every register
bdm dump 0xfffa00 64          # memory; --output FILE for raw bytes
bdm write 0x100000 0x4e 0x71  # bytes; --size word|long
bdm go                        # run from RPC
bdm halt                      # stop a running target
bdm reset --run               # reset to run, background mode off
bdm clock 2000000             # DSCLK; without a rate, show it
bdm help                      # the rest
```

`--pod usb`, the default, opens the only pod connected; with more than
one, `bdm` lists their serial numbers and stops. `--pod usb:SERIAL`
picks one by the serial number `bdm list` shows (the Pico's unique ID,
as picotool shows it; case ignored). `bdm list` opens no pod, so it
needs no udev rule. `--pod unix:PATH` or `tcp:HOST:PORT` reaches a
stand-in pod ([development.md](development.md)).

Keep DSCLK at or below about a third of the target's system clock
([status.md](status.md)).

## gdb

`bdm gdb --scratch ADDRESS` serves one gdb session on 127.0.0.1:1234
(`--listen HOST:PORT` for another). In an m68k gdb (`gdb-multiarch`):

```
set architecture m68k:cpu32
set endian big
target remote localhost:1234
```

Without an executable gdb does not know the CPU32 is big-endian, and
shows every register and word byte-reversed: set the endianness.

- `--scratch` names 1026 bytes of RAM, even, that the program does not
  use: a step's exception table and BGND stub.
- Breakpoints are BGND written into memory, so RAM only.
- Detach leaves the target running; kill leaves it halted.
- Ctrl-C in gdb halts the target. SIGINT or SIGTERM to `bdm` ends the
  session: breakpoints removed, the target halted, SFC and DFC put
  back, exit status 130 or 143. A second signal exits at once.
- gdb closing the connection without detaching or killing
  (`disconnect`, or gdb exiting) ends the session: breakpoints removed,
  the target halted, SFC and DFC put back, exit status 0.

## Python

The module `mc683xx_bdm` (`pip install mc683xx-bdm`, Python 3.12 or
later) keeps one pod open across calls, for scripts that loop and
poll.

```python
import time
import mc683xx_bdm

with mc683xx_bdm.Pod("usb") as pod:  # or "usb:SERIAL", "unix:/tmp/pod.sock"
    print(pod.reset_with_breakpoint())      # Entry.Breakpoint
    pod.write_register("PC", 0x100000)
    pod.write_memory(0x100000, bytes.fromhex("4e714afa"), size=2)
    pod.go()
    while pod.background is None:           # poll until it halts
        time.sleep(0.01)
    print(pod.registers())
    print(hex(pod.bus_read(0xFFFA00, 2)))   # SIMCR
```

- The names follow the emulator's bindings (m683xx-emu's
  `crates/cpu32-py`): `halt`, `go`, `reset_with_breakpoint`,
  `background`, `read_register("D0")`, `registers()`,
  `bus_read(address, size, function_code=5)`, `Entry`, `BusError`,
  `TargetError`.
- Blocks: `read_memory(address, length, *, size=1, function_code=5)`
  and `write_memory(address, data, ...)`.
- The pod's own: `pins`, `version`, `hello`, `set_clock`, `get_clock`,
  `resync`, `call`, `reset` (to run) and `transfer`.
- Leaving the `with` block, or `close()`, puts back the program's SFC
  and DFC and leaves the target as it is. A `Pod` collected unclosed
  does the same, best effort, with a `ResourceWarning`.
- Exceptions: `TransportError` (an `OSError`), `ProtocolError`,
  `TargetError`, `BusError`, and `ValueError` for a bad name, size or
  function code.
- Types: `crates/py/python/mc683xx_bdm/__init__.pyi`.

`serve_gdb` serves one gdb session over the open pod, after a script
has set the target up (chip selects, say). It returns
`Ending.DETACHED` (running), `Ending.KILLED` (halted) or
`Ending.HUNG_UP` (gdb closed the connection without detaching or
killing; breakpoints removed, halted):

```python
with mc683xx_bdm.Pod("usb") as pod:
    pod.reset_with_breakpoint()
    ...                                     # the board's own setup
    ending = pod.serve_gdb("127.0.0.1:1234", ready=print, scratch=0x100800)
```

`scratch` is required; the address defaults to 127.0.0.1:1234, and
`ready(port)` is called once it listens. It blocks with the GIL
released. Ctrl-C (`KeyboardInterrupt`, or any signal handler's
exception) ends the session, breakpoints removed and the target
halted, and is raised; a failure to clean up is its `__cause__`.
Python leaves SIGTERM to its default, which ends the process with
nothing removed: a script that may get one installs a handler that
raises.
