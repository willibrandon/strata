# Strata instruction set and privileged architecture

The Strata CPU ("S1") implements **RV32I** (base integer ISA, version 2.1),
**Zicsr**, **Zifencei**, and the subset of the RISC-V privileged architecture
(version 1.12) listed here. This document is normative for the hardware:
anything not described here is not implemented.

`IALIGN = 32` (no compressed instructions), `XLEN = 32`, one hart
(`mhartid = 0`), little-endian, physical addressing only.

## Privilege modes

Two modes are implemented: **M** (machine, encoding 3) and **U** (user,
encoding 0). Reset enters M-mode. There is no S-mode and no virtual memory.

## Unprivileged instructions

All 37 RV32I computational, control-transfer, load/store instructions, plus
`FENCE`, `FENCE.I`, `ECALL`, `EBREAK`, and the six Zicsr instructions.

* `FENCE` (any predecessor/successor set, including `FENCE.TSO`) executes as a
  no-op: the pipeline is in-order with at most one outstanding data access, so
  all memory and I/O accesses are already performed in program order.
* `FENCE.I` discards all younger instructions and refetches, so stores that
  precede it are visible to instruction fetches that follow it.
* Loads and stores must be naturally aligned; see exceptions below.

An instruction is **illegal** (cause 2) if it is not exactly one of the
encodings above or `MRET`/`WFI`, including: any encoding whose low two bits
are not `11`; reserved `funct3`/`funct7` values; shift-immediate encodings with
a nonzero `imm[11:5]` other than `0100000` for `SRAI`; `ECALL`, `EBREAK`,
`MRET`, `WFI` with nonzero `rd`/`rs1`/unused fields; and CSR accesses that
violate the rules below. The all-zeros and all-ones words are illegal.
`FENCE`/`FENCE.I` ignore their `rd`, `rs1`, and immediate fields, as the
specification requires.

## Privileged instructions

| Instruction | Behavior |
|-------------|----------|
| `MRET`      | M-mode only (illegal in U-mode). `mode <- MPP`, `MIE <- MPIE`, `MPIE <- 1`, `MPP <- U`; if the new mode is U, `MPRV <- 0`. `pc <- mepc`. |
| `WFI`       | Illegal in U-mode when `mstatus.TW = 1`. Otherwise stalls until `(mip & mie) != 0` (independent of `mstatus.MIE`), then completes. If an interrupt is then enabled globally it is taken with `mepc` = the instruction after `WFI`. |
| `ECALL`     | Raises environment-call exception (cause 8 from U, 11 from M). |
| `EBREAK`    | Raises breakpoint exception (cause 3). |

`SFENCE.VMA`, `SRET`, `DRET`, and all other SYSTEM encodings are illegal.

## CSRs

A CSR instruction is illegal if the CSR does not exist, if the current mode is
lower than the CSR's privilege level (`csr[9:8]`), if it writes a read-only CSR
(`csr[11:10] = 11`), or if it accesses a counter from U-mode whose `mcounteren`
bit is 0. `CSRRW`/`CSRRWI` always write; `CSRRS`/`CSRRC` write iff `rs1 != x0`;
`CSRRSI`/`CSRRCI` write iff `uimm != 0`. `CSRRW`/`CSRRWI` with `rd = x0` do not
read (no difference for these CSRs: none have read side effects).

Any CSR instruction that writes a CSR serializes the pipeline: younger
instructions are discarded and refetched, so they observe the new value.

| Address | Name | Access | Implementation |
|---------|------|--------|----------------|
| `0x300` | `mstatus`  | MRW | `MIE`[3], `MPIE`[7], `MPP`[12:11], `MPRV`[17], `TW`[21]. All other bits read 0. `MPP` is WARL: writing `11` stores M, any other value stores U (`00`). |
| `0x301` | `misa`     | MRW | read-only `0x4010_0100` (MXL = 1, extensions I and U). Writes ignored. |
| `0x304` | `mie`      | MRW | `MSIE`[3], `MTIE`[7], `MEIE`[11]; other bits read 0. |
| `0x305` | `mtvec`    | MRW | `BASE`[31:2], `MODE`[0]: 0 = direct, 1 = vectored (interrupt `i` jumps to `BASE + 4*i`). Bit 1 reads 0. |
| `0x306` | `mcounteren` | MRW | `CY`[0], `TM`[1], `IR`[2]; other bits read 0. |
| `0x30A` | `menvcfg`  | MRW | reads 0, writes ignored. |
| `0x310` | `mstatush` | MRW | reads 0, writes ignored. |
| `0x31A` | `menvcfgh` | MRW | reads 0, writes ignored. |
| `0x320` | `mcountinhibit` | MRW | reads 0, writes ignored (counters always count). |
| `0x323`-`0x33F` | `mhpmevent3`-`31` | MRW | read 0, writes ignored. |
| `0x340` | `mscratch` | MRW | 32-bit register. |
| `0x341` | `mepc`     | MRW | bits [31:2]; bits [1:0] read 0. |
| `0x342` | `mcause`   | MRW | `Interrupt`[31], `Code`[4:0]; other bits read 0. |
| `0x343` | `mtval`    | MRW | 32-bit register. |
| `0x344` | `mip`      | MRW | `MSIP`[3], `MTIP`[7], `MEIP`[11] are read-only reflections of the TIMER and INTC; writes ignored. |
| `0x3A0` | `pmpcfg0`  | MRW | configuration of PMP entries 0-3 (see PMP). |
| `0x3A1`-`0x3A3` | `pmpcfg1`-`3` | MRW | read 0, writes ignored. |
| `0x3B0`-`0x3B3` | `pmpaddr0`-`3` | MRW | 32-bit, holds address bits [33:2]. |
| `0x3B4`-`0x3BF` | `pmpaddr4`-`15` | MRW | read 0, writes ignored. |
| `0xB00` | `mcycle`   | MRW | low half of a 64-bit cycle counter (increments every clock). |
| `0xB02` | `minstret` | MRW | low half of a 64-bit retired-instruction counter. |
| `0xB03`-`0xB1F` | `mhpmcounter3`-`31` | MRW | read 0, writes ignored. |
| `0xB80` | `mcycleh`  | MRW | high half of `mcycle`. |
| `0xB82` | `minstreth`| MRW | high half of `minstret`. |
| `0xB83`-`0xB9F` | `mhpmcounter3h`-`31h` | MRW | read 0, writes ignored. |
| `0xC00`, `0xC80` | `cycle`, `cycleh` | URO | `mcycle`; U-mode needs `mcounteren.CY`. |
| `0xC01`, `0xC81` | `time`, `timeh` | URO | TIMER `mtime`; U-mode needs `mcounteren.TM`. |
| `0xC02`, `0xC82` | `instret`, `instreth` | URO | `minstret`; U-mode needs `mcounteren.IR`. |
| `0xC03`-`0xC1F`, `0xC83`-`0xC9F` | `hpmcounter*` | URO | read 0; U-mode access is illegal (the `mcounteren` bits are 0). |
| `0xF11` | `mvendorid` | MRO | 0 |
| `0xF12` | `marchid`   | MRO | 0 |
| `0xF13` | `mimpid`    | MRO | `0x0001_0000` |
| `0xF14` | `mhartid`   | MRO | 0 |
| `0xF15` | `mconfigptr`| MRO | 0 |

Every other CSR address is illegal.

A write to `mcycle[h]` or `minstret[h]` replaces that half of the counter; the
counter does not also increment in that cycle, and an instruction that writes
`minstret` is not counted. `minstret` counts instructions that retire without
a trap. Reads of a counter observe the value before the reading instruction's
own effects.

## Traps

### Exception causes

| Code | Exception | `mtval` |
|------|-----------|---------|
| 0 | Instruction address misaligned | target address |
| 1 | Instruction access fault | faulting instruction address |
| 2 | Illegal instruction | the instruction word |
| 3 | Breakpoint | address of the `EBREAK` |
| 4 | Load address misaligned | effective address |
| 5 | Load access fault | effective address |
| 6 | Store address misaligned | effective address |
| 7 | Store access fault | effective address |
| 8 | Environment call from U-mode | 0 |
| 11 | Environment call from M-mode | 0 |

Instruction-address-misaligned is raised by the taken branch or jump whose
target has bit 1 set (`JALR` clears bit 0 as specified); the jump does not
write `rd`. Access faults come from PMP checks and from the memory map
(unmapped addresses, stores to ROM, fetches from non-executable regions,
sub-word MMIO accesses). All exceptions are **precise**: the trapping
instruction and all younger instructions have no architectural effect, and no
bus access is made for a load or store that traps for misalignment or PMP.
If a load or store raises an access fault from the memory map, the device is
not accessed.

### Interrupts

| Code | Interrupt | Source |
|------|-----------|--------|
| 3  | Machine software (`MSIP`) | TIMER `MSIP` register |
| 7  | Machine timer (`MTIP`) | `mtime >= mtimecmp` |
| 11 | Machine external (`MEIP`) | INTC |

An interrupt `i` is taken when `mip[i] & mie[i]` and either the hart is in
U-mode or `mstatus.MIE = 1`. Priority when several are pending: MEI, MSI,
MTI. Interrupts are taken between instructions: `mepc` is the address of the
first instruction that has not been executed.

### Trap entry and return

On a trap: `mepc <- pc of the trapping/interrupted instruction`, `mcause`,
`mtval` as above, `MPIE <- MIE`, `MIE <- 0`, `MPP <- current mode`,
`mode <- M`, `pc <- mtvec.BASE` (plus `4*code` for interrupts when
`mtvec.MODE = 1`). All younger instructions are discarded.

## Physical memory protection (PMP)

Four PMP entries (0-3) are implemented with granularity G = 0 (4 bytes).
`pmpcfg0` byte `i` configures entry `i`: `R`[0], `W`[1], `X`[2], `A`[4:3]
(0 OFF, 1 TOR, 2 NA4, 3 NAPOT), `L`[7]; bits [6:5] read 0. The combination
`R = 0, W = 1` is reserved: a write with that combination stores `W = 0`.

* TOR entry `i` matches addresses `a` with `pmpaddr[i-1] <= a[33:2] < pmpaddr[i]`
  (`pmpaddr[-1] = 0`). NA4 matches `a[33:2] = pmpaddr[i]`. NAPOT with `k`
  trailing ones in `pmpaddr[i]` matches the naturally aligned `2^(k+3)`-byte
  region. Physical addresses are 32 bits, so `a[33:32] = 0`.
* The lowest-numbered matching entry decides. U-mode accesses require the
  entry's permission bit; M-mode accesses are allowed unless the entry is
  locked (`L = 1`) and lacks the permission.
* If no entry matches, M-mode accesses succeed and U-mode accesses fail.
* A locked entry's `pmpcfg` byte and `pmpaddr` are read-only until reset;
  `pmpaddr[i]` is also read-only when entry `i+1` is locked and is TOR.
* Instruction fetches are checked with the current mode and `X`. Loads and
  stores are checked with `R`/`W` and the effective mode: `MPP` if
  `mstatus.MPRV = 1` and the hart is in M-mode, otherwise the current mode.
* A PMP violation raises the access-fault exception for the access type.
  Because all accesses are naturally aligned and at most 4 bytes, an access
  never spans two 4-byte granules.

Reset clears all PMP entries (`A = OFF`, `L = 0`).

## Reset state

`pc = 0x0000_0000`, mode M, `mstatus.MIE = 0`, `MPRV = 0`, `TW = 0`,
`mie = 0`, `mtvec = 0`, `mcounteren = 0`, `mcause = 0`, PMP entries off,
`mcycle = minstret = 0`. General-purpose registers `x1`-`x31`, `mscratch`,
`mepc`, and `mtval` are **undefined** after reset; software must not depend
on their values.
