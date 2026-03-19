# ic20c

IC20 is a compiled language for [Stationeers](https://store.steampowered.com/app/544550/Stationeers/) programmable chips. It compiles to IC10 assembly.

IC20 is a statically typed language with Rust-inspired syntax. It gives you named variables, functions, structured control flow, and type checking, and compiles everything down to tight IC10 assembly that fits within the chip's 128-line limit.

## Quick Start

```sh
ic20c hello.ic20
```

Output goes to stdout by default, so you can pipe it or redirect it:

```sh
ic20c hello.ic20 -o hello.ic10
```

Paste the resulting IC10 assembly into a programmable chip in Stationeers.

## Example

```rust
device sensor: d0;
device heater: d1;

const SETPOINT: f64 = 300.0;

fn main() {
    loop {
        let temp = sensor.Temperature;
        heater.On = select(temp < SETPOINT, 1.0, 0.0);
        yield;
    }
}
```

This compiles to a handful of IC10 instructions that read a temperature sensor every game tick and toggle a heater on or off.

More examples are in the [`examples/`](examples/) directory.

## Building

```sh
cargo build --release
```

The binary will be at `target/release/ic20c` (or `target/release/ic20c.exe` on Windows).

## Language Overview

Everything at runtime is a 64-bit float — that's what IC10 has. IC20 gives you three compile-time types on top of that: `f64`, `bool`, and `i53`. The `i53` name reflects the nature of IEEE 754 doubles, which are can exactly represent integers up to that size.

Variables are declared with `let` and are immutable by default. Use `let mut` if you need reassignment. `const` is for compile-time constants folded into the output; `static` is for values that persist across ticks (stored in IC10's `r` registers rather than spilled to the stack).

Control flow is `if`/`else`, `loop`, `while`, and `for`/`in` with ranges (`0..10`, `0..=10`). `break` and `continue` work as expected and accept optional labels for breaking out of nested loops.

Functions take up to 8 parameters, passed in registers per the IC10 calling convention. Recursion works. Forward references are allowed, so you can call a function before it's defined in the file.

Device I/O uses named declarations: `device sensor: d0;` binds the name `sensor` to pin `d0`. After that, reads and writes use dot notation — `sensor.Temperature`, `heater.On = 1.0`. Slot access is `device.slot(n).Field`. For network-wide batch reads and writes, use `batch_read` and `batch_write` with a hash of the device type name.

Built-in functions cover the math intrinsics IC10 exposes: `sin`, `cos`, `sqrt`, `abs`, `min`, `max`, `lerp`, `clamp`, `rand`, and others. `select(cond, a, b)` is a branchless ternary. `hash("TypeName")` computes a CRC-32 at compile time, which is what IC10 uses for batch device addressing. `is_nan` checks for NaN, useful when reading from an unconnected device pin.

## Compiler Flags

```shell
ic20c [OPTIONS] <SOURCE>
```

| Flag | Description |
| --- | --- |
| `-o <FILE>` | Write output to a file instead of stdout |
| `-O <LEVEL>` | Optimization level: `0`, `g`, `1`, `2` (default: `2`) |
| `-f <FEATURE>` | Enable/disable an optimization pass (e.g. `-f no-inline`) |
| `--dump-ast` | Dump the AST after parsing and exit |
| `--dump-resolved` | Dump the bound IR after name resolution and exit |
| `--dump-cfg` | Dump the CFG before SSA and exit |
| `--dump-ssa` | Dump the SSA form before optimization and exit |
| `--dump-regmap` | Dump the register allocation map and exit |

### Optimization Levels

| Level | Behavior |
| --- | --- |
| `-O0` | Block simplifications only. Symbolic labels in output. |
| `-Og` | Single pass of all optimizations, no inlining. Symbolic labels. Useful when you need readable output to debug a codegen problem. |
| `-O1` | Single pass of all optimizations including inlining. Numeric jump targets. |
| `-O2` | Full fixpoint optimization loop. Numeric jump targets. Default. |

The 128-line limit is easy to bump into once your program gets non-trivial. `-O2` can make a meaningful difference in output size, so it's the default.

### Feature Toggles

Individual passes can be toggled with `-f <name>` (enable) or `-f no-<name>` (disable):

`constant-propagation`, `algebraic-simplification`, `copy-propagation`, `global-value-numbering`, `dead-code-elimination`, `block-simplification`, `block-deduplication`, `inline`, `branch-fusion`, `ic10-simplification`, `static-access`, `loop-invariant-code-motion`, `symbolic-labels`, `sccp`
