# Strata bus protocol

The CPU has two bus masters, the instruction bus (read-only) and the data
bus. Both use the same protocol. Each master connects to a decoder
(`st_ibus`, `st_dbus`) that routes commands to memory-macro ports and, for
the data bus, to the MMIO device pages. There is no shared bus and no
arbitration: every memory that both masters can reach (ROM, RAM) has one
port per master, and the video scan-out uses the second port of VRAM and
font RAM.

## Signals

| Signal | Direction | Width | Meaning |
|--------|-----------|-------|---------|
| `cmd_valid` | master to slave | 1 | a command is offered |
| `cmd_addr`  | master to slave | 32 | byte address |
| `cmd_we`    | master to slave | 1 | write (data bus only) |
| `cmd_be`    | master to slave | 4 | byte lanes: bytes written, or bytes requested by a read |
| `cmd_wdata` | master to slave | 32 | write data, already placed in its byte lanes |
| `cmd_ready` | slave to master | 1 | the command is accepted this cycle |
| `rsp_valid` | slave to master | 1 | a response is delivered this cycle |
| `rsp_rdata` | slave to master | 32 | read data (all four lanes) |
| `rsp_err`   | slave to master | 1 | the access failed and had no effect |

## Rules

1. A command transfers in a cycle with `cmd_valid && cmd_ready`.
2. Every command receives exactly one response, in a later cycle, with
   `rsp_valid` high for one cycle. Responses return in command order.
3. A master has at most one outstanding command. It may transfer a new
   command in the same cycle in which the previous command's response
   arrives, so a slave that always responds in the next cycle sustains one
   command per cycle.
4. A master always accepts a response (there is no `rsp_ready`).
5. `cmd_valid` may depend combinationally on the `rsp_*` signals of the same
   cycle. Slaves register their responses: there is no combinational path
   from `cmd_*` to `rsp_*`, and `cmd_ready` does not depend on `cmd_valid`.
6. A response with `rsp_err` means the access did not happen: a failed write
   changed nothing and a failed read had no side effects.
7. Byte lanes are fixed: lane `i` is `cmd_wdata[8i+7:8i]` / `rsp_rdata[8i+7:8i]`
   and corresponds to address `(cmd_addr & ~3) + i`.

## Timing in this implementation

Every slave in the machine has `cmd_ready = 1` and responds exactly one cycle
after the command: the memory macros are synchronous SRAMs with registered
outputs, and each MMIO device registers its read data. The protocol still
allows slower slaves, and the CPU handles any response latency.

```
 cycle        0        1        2        3
 cmd_valid  __/‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾\______
 cmd_addr     A0       A1       A2
 rsp_valid  ___________/‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾\
 rsp_rdata             D0       D1       D2
```

## How the CPU uses the data bus

The data-bus command for a load or store is issued from the M stage, and only
in a cycle in which the instruction also moves to W. It is issued only after
the previous access has returned without error, which is visible
combinationally through rule 5. As a result a store that follows a faulting
access is never sent, and bus errors are precise exceptions
(see [microarchitecture.md](microarchitecture.md)).
