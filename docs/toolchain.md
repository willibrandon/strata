# Strata software toolchain

```
 C source ──clang -S──▶ .s ─┐
 .S source ──clang -E──▶ .s ─┼─▶ strata-as ──▶ linked program (.bin/.hex/.elf) + debug info (.sdb)
 hand-written .s ───────────┘                     │
                                    strata-pack ◀─┘──▶ boot image (.img) for the boot flash
```

* **strata-as** (`tools/crates/strata-asm`) is Strata's own assembler *and*
  linker. It reads all input files of one program, lays them out according to
  a layout file, resolves every relocation itself, and writes the outputs. It
  never invokes another assembler or linker.
* **clang** is used only as a C-to-assembly translator (`-S`) and as the C
  preprocessor for `.S` files (`-E`). No LLVM assembler, linker, `objcopy`, or
  runtime library is used to build guest software. `llvm-mc`, `ld.lld` and
  `llvm-objdump` are used only by tests, to cross-check strata-as.
* **strata-pack** builds boot images from linked programs.

C compilation flags (see `sw/mk/rules.mk`):

```
clang --target=riscv32-unknown-elf -march=rv32i_zicsr_zifencei -mabi=ilp32
      -mno-relax -mcmodel=medlow -fno-pic -ffreestanding -nostdlib
      -fno-asynchronous-unwind-tables -fno-unwind-tables -O2 -gline-tables-only -S
```

## strata-as

```
strata-as [options] <input.s>... 
  -T, --layout <file>     layout file (required unless --org is given)
      --org <addr>        simple layout: everything contiguous from <addr>
                          (.text*, .rodata*, .data*, .bss*, COMMON in that order)
  -o, --elf <file>        ELF32 executable (EM_RISCV, with section headers and .symtab)
      --bin <file>        flat binary: all non-NOBITS bytes from the lowest to the
                          highest loaded address, gaps filled with 0
      --hex <file>        like --bin, one little-endian 32-bit word per line, hex
                          (suitable for $readmemh); padded with zeros to a word
      --debug <file>      debug information (JSON, see below)
      --listing <file>    listing: address, encoding, source text per line
      --map <file>        placement of every input section and symbol
      --gc-sections       remove input sections not reachable from the entry
                          symbol, KEEP() layout patterns or --keep-symbol
      --keep-symbol <sym> with --gc-sections, also keep the section defining
                          <sym> (repeatable)
  -I <dir>                search directory for .include / .incbin
  -D <sym>=<value>        define an absolute symbol before assembly
```

Diagnostics are printed as `file:line:col: error: message` and the exit status
is nonzero on any error. Warnings do not fail the build.

### Source syntax

GNU `as` syntax for RISC-V, as emitted by clang and as commonly hand-written.

* One statement per line; `;` also separates statements. `#` starts a comment
  anywhere except inside a string or character literal, **except** that a line
  of the form `# <number> "<file>" [flags...]` (a C preprocessor line marker)
  sets the current source file and line for diagnostics and debug info.
  `/* ... */` comments (possibly multi-line) are accepted.
* Labels: `name:` (identifiers may contain `A-Za-z0-9_.$`), and numeric local
  labels `N:` referenced as `Nb` / `Nf`.
* Expressions: integer literals (decimal, `0x`, `0b`, leading-`0` octal),
  character literals (`'a'`, with `\n \t \\ \' \0` escapes), symbols, `.` (the
  current location), unary `- ~ !`, binary `* / % << >> & | ^ + -` and
  comparisons `== != < > <= >=` with C precedence, and parentheses. The result
  is either absolute, or relocatable (`symbol + constant`), or the difference
  of two symbols defined in the same section (absolute).
* Relocation operators: `%hi(expr)`, `%lo(expr)`, `%pcrel_hi(expr)`,
  `%pcrel_lo(label)` (where `label` marks the paired `auipc`).
* Registers: `x0`-`x31`, ABI names (`zero ra sp gp tp t0-t6 s0-s11 fp a0-a7`).
* CSRs: all names in [isa.md](isa.md) plus any 12-bit number.

### Instructions

All RV32I, Zicsr, Zifencei instructions, `mret`, `wfi`, `ecall`, `ebreak`,
`fence [pred, succ]` (default `iorw, iorw`), `fence.tso`, `fence.i`. Memory
operands are `offset(reg)`, `(reg)`, or `%lo(sym)(reg)`. `jalr` accepts
`jalr rs`, `jalr rd, rs, imm`, and `jalr rd, imm(rs)`; `jal label` means
`jal ra, label`.

Pseudo-instructions and their exact expansions:

| Pseudo | Expansion |
|--------|-----------|
| `nop` | `addi x0, x0, 0` |
| `li rd, imm` | `addi rd, x0, imm` if `imm` fits in 12 bits signed; otherwise `lui rd, hi` then `addi rd, rd, lo` (the `addi` is omitted when `lo = 0`), where `hi = (imm + 0x800) >> 12`, `lo = imm - (hi << 12)`. `imm` must be an absolute expression known at that point. |
| `la rd, sym` / `lla rd, sym` | `1: auipc rd, %pcrel_hi(sym)` ; `addi rd, rd, %pcrel_lo(1b)` |
| `mv rd, rs` | `addi rd, rs, 0` |
| `not rd, rs` | `xori rd, rs, -1` |
| `neg rd, rs` | `sub rd, x0, rs` |
| `seqz rd, rs` | `sltiu rd, rs, 1` |
| `snez rd, rs` | `sltu rd, x0, rs` |
| `sltz rd, rs` | `slt rd, rs, x0` |
| `sgtz rd, rs` | `slt rd, x0, rs` |
| `beqz/bnez/bltz/bgez rs, L` | `beq/bne/blt/bge rs, x0, L` |
| `blez rs, L` | `bge x0, rs, L` |
| `bgtz rs, L` | `blt x0, rs, L` |
| `bgt/ble/bgtu/bleu rs, rt, L` | `blt/bge/bltu/bgeu rt, rs, L` |
| `j L` | `jal x0, L` |
| `jr rs` | `jalr x0, 0(rs)` |
| `ret` | `jalr x0, 0(ra)` |
| `call sym` | `auipc ra, %pcrel_hi(sym)` ; `jalr ra, %pcrel_lo(...)(ra)` |
| `call rd, sym` | same with `rd` in place of `ra` |
| `tail sym` | `auipc t1, %pcrel_hi(sym)` ; `jalr x0, %pcrel_lo(...)(t1)` |
| `jump sym, rt` | `auipc rt, %pcrel_hi(sym)` ; `jalr x0, %pcrel_lo(...)(rt)` |
| `l{b,h,w,bu,hu} rd, sym` | `auipc rd, %pcrel_hi(sym)` ; `l* rd, %pcrel_lo(...)(rd)` |
| `s{b,h,w} rs, sym, rt` | `auipc rt, %pcrel_hi(sym)` ; `s* rs, %pcrel_lo(...)(rt)` |
| `csrr rd, csr` | `csrrs rd, csr, x0` |
| `csrw/csrs/csrc csr, rs` | `csrrw/csrrs/csrrc x0, csr, rs` |
| `csrwi/csrsi/csrci csr, imm` | `csrrwi/csrrsi/csrrci x0, csr, imm` |
| `rdcycle[h]/rdtime[h]/rdinstret[h] rd` | `csrrs rd, cycle[h]/time[h]/instret[h], x0` |
| `unimp` | `csrrw x0, cycle, x0` (`0xC0001073`) |

Instruction operands that do not fit their field are errors. Branch and jump
targets must be in range; strata-as does not relax branches.

### Directives

| Directive | Meaning |
|-----------|---------|
| `.text`, `.data`, `.bss` | switch to that section |
| `.section name[, "flags"[, @type[, entsize]]]` | switch to (and create) a section. Flags `a w x M S` and types `@progbits @nobits` are honored for ELF output; a section named `.bss*`/`.sbss*` or with `@nobits` holds no file bytes. |
| `.pushsection name ...`, `.popsection`, `.previous` | section stack |
| `.byte .2byte .half .short .4byte .word .long .8byte .quad .dword` | data; `.word`/`.4byte`/`.long` accept relocatable expressions (absolute 32-bit relocation) |
| `.ascii .asciz .string` | strings (C escapes), `.asciz`/`.string` NUL-terminate |
| `.zero n`, `.space n[, fill]`, `.skip n[, fill]`, `.fill count[, size[, value]]` | fill |
| `.align n`, `.p2align n[, fill[, max]]`, `.balign n[, fill[, max]]` | alignment (`.align` is a power of two, as on RISC-V). In a section containing instructions, padding without an explicit fill is `nop`s (whole words) after any needed zero bytes. |
| `.globl .global .local .weak` | binding |
| `.type sym, @function/@object/%function/%object`, `.size sym, expr` | symbol metadata (kept in debug info and ELF) |
| `.equ sym, expr`, `.set sym, expr`, `sym = expr` | define an absolute (or relocatable) symbol |
| `.comm sym, size[, align]`, `.lcomm sym, size[, align]` | common/bss symbols, allocated into the output section whose rule lists `COMMON` |
| `.incbin "file"[, skip[, count]]` | include raw bytes |
| `.include "file"` | include source |
| `.macro name [arg[=default]] ...` / `.endm`, `.exitm` | macros; `\arg` substitutes, `\@` is a per-expansion counter |
| `.rept n` / `.endr`, `.irp sym, values...` / `.endr` | repetition |
| `.if expr`, `.ifdef sym`, `.ifndef sym`, `.else`, `.elseif expr`, `.endif` | conditional assembly |
| `.error "msg"`, `.warning "msg"` | diagnostics |
| `.file N "dir" "name" [md5 x]`, `.file N "name"` | declare debug file N |
| `.loc file line [col] [flags...]` | source position of following code |
| `.file "name"`, `.ident`, `.attribute`, `.option ...`, `.cfi_*`, `.addrsig`, `.addrsig_sym`, `.hidden`, `.protected`, `.internal` | accepted and ignored (`.option push/pop` are tracked for balance) |

Sections whose names begin with `.debug`, `.comment`, `.note`, `.llvm`, or
`.riscv.attributes` are discarded, including any contents and label
references inside them.

### Layout files

A layout file places input sections into output sections within named memory
regions. Example (`sw/layout/kernel.ld`):

```
# comments start with '#'
memory RAM  origin=0x10000000 length=16K
entry _start

output .text   in RAM { .text.start .text .text.* }
output .rodata in RAM align=4 { .rodata .rodata.* .srodata .srodata.* }
output .data   in RAM align=4 { .data .data.* .sdata .sdata.* }
output .bss    in RAM align=4 nobits { .bss .bss.* .sbss .sbss.* COMMON }

define __bss_start = START(.bss)
define __bss_end   = END(.bss)
define __stack_top = ORIGIN(RAM) + LENGTH(RAM)
```

* `memory NAME origin=A length=L` (`L` may use `K`/`M` suffixes).
* `output NAME in REGION [at=ADDR] [align=N] [nobits] { patterns... }`.
  Output sections are placed in file order, each after the previous one in
  the same region (or at `at=`), aligned to `max(align, largest input
  alignment)`. Patterns are globs (`*`, `?`) over input section names; an
  input section goes to the first output section with a matching pattern.
  Input sections keep command-line file order within an output section.
* A pattern may be written `KEEP(pattern)` (several patterns may share one
  `KEEP(...)`), e.g. `output .text in SLOT { KEEP(.text.header) .text .text.* }`.
  It matches and places exactly like the plain pattern, and additionally
  makes the matching input sections roots for `--gc-sections`.
  `KEEP(COMMON)` keeps all common symbols.
* `define SYM = EXPR` where `EXPR` may use numbers, defined symbols,
  `START(sec)`, `END(sec)`, `SIZE(sec)`, `ORIGIN(region)`, `LENGTH(region)`,
  `ALIGN(expr, n)`, and `+ - * / & |`.
* `entry SYM` names the entry point.
* An input section with contents that matches no rule, or content that
  overflows its region, is an error.

With `--gc-sections`, only input sections reachable from the roots are
linked. The roots are the section defining the entry symbol (`entry SYM`,
or `_start` if the layout names none), every input section matched by a
`KEEP` pattern, and the sections defining the `--keep-symbol` symbols. A
live section makes another section live when any of its relocations or
symbol references resolves to it: instruction operands, `%hi`/`%lo`,
`%pcrel_hi`/`%pcrel_lo`, `call`/`la`/load/store pseudo-instructions, data
words, and both symbols of a label difference. References are followed
through `.set` symbols, common symbols (each is its own section), and
`define`s that name program symbols. NOBITS sections take part like any
other. Removed sections do not appear in any output (image, ELF, listing,
`.sdb` symbols and line tables); the map lists them under "Discarded by
--gc-sections". References made only from removed sections are never
resolved, so they may be undefined, and only live sections need a matching
output rule. It is an error if there are no roots.

### Debug information (`.sdb`)

JSON, consumed by the debugger:

```json
{
  "format": "strata-debug", "version": 1,
  "entry": 268435456,
  "sections": [ {"name": ".text", "addr": 268435456, "size": 1234, "flags": "ax", "nobits": false} ],
  "symbols":  [ {"name": "main", "addr": 268435600, "size": 88, "kind": "func",
                 "bind": "global", "section": ".text", "file": 3} ],
  "files":    [ "sw/kernel/main.c", "build/kernel/main.s" ],
  "lines":     [ [268435600, 0, 42, 5, 1] ],
  "asm_lines": [ [268435600, 1, 117] ]
}
```

* `kind` is `func`, `object`, `section`, or `notype`; `bind` is `global`,
  `local`, or `weak`. Local labels starting with `.L` are omitted. `file` is
  the index of the input file that defined the symbol.
* `files` are paths as given on the command line or in `.file`/line markers
  (joined with the `.file` directory when that is relative).
* `lines`: `[addr, file, line, column, is_stmt]` rows, sorted by address, from
  `.loc` directives: each row applies from `addr` up to the next row's
  address. Only addresses of emitted instructions are covered.
* `asm_lines`: `[addr, file, line]`, one row per emitted instruction (for a
  pseudo-instruction, each expanded instruction), giving the location in the
  assembler input (after line markers; for macro expansions, the invocation
  line).

## Boot image format

A boot image holds one or more segments to be copied to memory, and an entry
point. All fields are little-endian 32-bit words.

| Offset | Field | Description |
|--------|-------|-------------|
| `0x00` | magic | `0x4152_5453` ("STRA") |
| `0x04` | version | 1 |
| `0x08` | entry | address to jump to (in M-mode) |
| `0x0C` | nseg | number of segments, 1-15 |
| `0x10` | hcrc | CRC-32 of bytes `[0, 0x14 + 12*nseg)` computed with this field = 0 |
| `0x14 + 12*i` | seg[i].addr | load address (4-byte aligned) |
| `0x18 + 12*i` | seg[i].len  | length in bytes (multiple of 4) |
| `0x1C + 12*i` | seg[i].crc  | CRC-32 of the segment's bytes |

The segment payloads follow the table, in order, without padding. CRC-32 is
the IEEE 802.3 CRC (reflected polynomial `0xEDB88320`, initial value
`0xFFFFFFFF`, final XOR `0xFFFFFFFF`; `crc("123456789") = 0xCBF43926`).

```
strata-pack -o system.img --entry <addr|symbol@file.sdb> <file.bin>@<addr> ...
```

`strata-pack` builds an image from flat binaries, trimming trailing zero
words of each segment (the loader does not zero memory; programs clear
their own `.bss`).
