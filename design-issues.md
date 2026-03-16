## 3. Indirect device addressing is not expressible

**Where:** §8, §8.1

IC10 supports indirect device register addressing via the `dr?` prefix: if `r0` holds `2`,
then `s dr0 On 1` is equivalent to `s d2 On 1`. This enables iterating over a set of device
pins with a loop:

```ic10
move r0 0          # 6-device loop: 5 instructions total
loop:
l r1 dr0 Temperature
# ...
add r0 r0 1
blt r0 6 loop
```

IC20 provides no syntax for indirect device addressing. A program that needs to process all
six device pins must either repeat the body six times (6× code size) or encode the selection
as a chain of `if`/`else` branches (7 `if`/`else` pairs ≈ 7 extra jump instructions). Both
alternatives are strictly worse than the hand-written ic10 loop.

**Required fix:** Add a syntax for indirect device access, for example:
`device_by_index(expr).Field` or `pin(expr).Field`, where the expression evaluates to an
integer in `[0, 5]` and maps to `d0`–`d5`. This compiles directly to IC10 `dr?` addressing.

---

## 4. Device-connected checks are not expressible

**Where:** §8.1

IC10 provides `sdse` / `sdns` (set-if-device-set / set-if-device-not-set) and the
corresponding branch instructions `bdse` / `bdns` to test whether a device pin has a physical
device connected at runtime. This is a fundamental robustness primitive used in almost every
non-trivial ic10 program.

There is no IC20 expression or statement that lowers to any of these four instructions. A
program that must conditionally handle a missing device cannot express the check at all; it
can only read the field (returning `0` for unconnected pins) and infer connectivity
indirectly, which is ambiguous.

**Required fix:** Add a built-in expression such as `device.connected` (a `bool`-typed device
field access) that compiles to `sdse` / `sdns`. Alternatively, add a function
`is_connected(dev)` as a special form. The conditional variants `bdse` / `bdns` will follow
naturally from branch fusion (see issue 1).

---

## 5. Batch operations are missing three variants

**Where:** §8.5

The IC10 batch instruction family has six instructions; IC20 exposes only two (`lb` via
`batch_read` and `sb` via `batch_write`). The four missing instructions have no IC20
equivalent:

| Missing IC20 coverage | IC10 instruction | What it does |
|---|---|---|
| Batch read filtered by name hash | `lbn` | Read from all devices of a type that also have a specific name |
| Batch write filtered by name hash | `sbn` | Write to all devices of a type with a specific name |
| Batch slot read | `lbs` / `lbns` | Read a slot field from all matching devices |
| Batch slot write | `sbs` / `sbns` | Write a slot field to all matching devices |

Without `lbn`/`sbn`, a program cannot distinguish between two devices of the same type by
name on the same network without iterating over direct connections. Without `lbs`/`sbs`, slot
information from batch-addressed devices is inaccessible entirely.

**Required fix:** Add `batch_read_named`, `batch_write_named`, `batch_read_slot`,
`batch_write_slot`, `batch_read_slot_named`, and `batch_write_slot_named` (or a parameter
extension to the existing `batch_read`/`batch_write` syntax) to cover all six IC10 batch
instructions.

---

## 6. Approximate equality has no language expression

**Where:** §5.5

IC10 provides `sap` / `sna` (set-if-approximately-equal / not-approximately-equal) and
`sapz` / `snaz` (zero variants), plus the corresponding branch forms `bap` / `bna` /
`bapz` / `bnaz`. Approximate equality is important because floating-point arithmetic rarely
produces exact results.

IC20 has no operator or built-in that maps to these instructions. A programmer who needs
approximate equality must hand-roll it from arithmetic (`abs(a - b) <= threshold`), which
costs 3–4 instructions (`sub`, `abs`, `sle`) against a single `sap` instruction, and
additionally needs correct handling of the relative-vs-absolute threshold semantics that
`sap` encapsulates.

**Required fix:** Add `approx_eq(a, b, c)` and `approx_ne(a, b, c)` built-in expressions
(and zero-variants `approx_zero(a, b)` / `not_approx_zero(a, b)`) with `f64` operands and
`bool` result, mapping directly to `sap`, `sna`, `sapz`, `snaz`. Branch fusion (issue 1)
will then reduce `if approx_eq(…)` to a single `bap` instruction.

---

## 8. Logical right shift is inaccessible

**Where:** §5.4

The `>>` operator maps exclusively to IC10 `sra` (arithmetic right shift, sign-extending).
IC10 also provides `srl` (logical right shift, zero-filling). There is no IC20 operator or
built-in that lowers to `srl`. A programmer who needs an unsigned right shift must simulate
it with masking, which costs additional instructions.

This matters for bit-manipulation programs that treat the 64-bit value as an unsigned
quantity — for example, when isolating a bit field in the upper half of a value.

**Required fix:** Add a distinct operator or built-in for logical right shift. One option is
an `srl(a, b)` built-in function (consistent with the IC10 mnemonic); another is a `>>>`
operator following Java/JavaScript precedent.

---

## 9. Bit-field extraction and insertion are not expressible

**Where:** §5 (no section covers this)

IC10 provides `ext` (extract a contiguous bit field from a register) and `ins` (insert a bit
field into a register). Both take a source value, a bit offset, and a field width, and execute
in a single instruction. The IC20 equivalent requires a sequence of shift and mask operations
costing 2–4 instructions per extract and 4–6 per insert.

Bit-field operations are common in IC10 programs that pack multiple logical values into a
single register to work around the 16-register limit.

**Required fix:** Add `bit_extract(val: i53, offset: i53, len: i53) -> i53` and
`bit_insert(base: i53, field: i53, offset: i53, len: i53) -> i53` built-ins mapping directly
to `ext` and `ins`.

---

## 10. The stack is inaccessible as a data structure

**Where:** §10.3 describes the stack; no IC20 syntax accesses it as data

IC10 exposes a 512-slot random-access memory via `push`, `pop`, `peek`, `poke`, `get`, and
`put`. Skilled ic10 programmers use this as a general-purpose array — storing lookup tables,
packing multiple values per chip tick, holding state across iterations — far beyond its use
as a call stack. Because IC10 has only 16 registers, the stack array is the only general
storage available.

IC20 currently uses the stack solely for register spilling and `ra` saves. There is no
programmatic access to `push`/`pop`/`poke`/`peek`/`get`/`put` from IC20 source. Any program
pattern requiring indexed persistent storage is simply inexpressible, forcing workarounds that
either blow the register budget or require restructuring the algorithm.

**Required fix:** Expose the stack as an addressable array. A minimal surface could be
built-in functions: `stack_push(val: f64)`, `stack_pop() -> f64`, `stack_peek() -> f64`,
`stack_poke(addr: i53, val: f64)`, and `stack_get(addr: i53) -> f64`. The interaction between
programmer-managed stack use and compiler-managed spill/call use would need to be specified
(the simplest approach: programmer stack access operates relative to a reserved region, or
the programmer takes full responsibility when opting in to direct stack use).

---

## 11. Reference-id device access is not expressible

**Where:** §8.1

IC10 supports `ld` / `sd` instructions that address a device by its `ReferenceId` — a
runtime integer that uniquely identifies a specific device instance. This allows a program to
store and later address devices by identity rather than by pin. There is no IC20 syntax for
this; the only device access mechanism binds names to fixed physical pins at compile time.

**Required fix:** Add a syntax for reference-id access, for example a built-in
`read_by_id(ref_id: f64, field: identifier) -> f64` and
`write_by_id(ref_id: f64, field: identifier, val: f64)` special forms mapping to `ld`/`sd`.

---

## 12. Network channel access is not expressible

**Where:** §8 (no section covers channels)

IC10 supports reading and writing network channels via the connection-index syntax
`d0:0 Channel3`. There is no IC20 syntax for channel access. Programs that communicate
over cable networks (a common Stationeers pattern) cannot be written in IC20 at all.

**Required fix:** Add channel access syntax, for example
`sensor.channel(conn_idx).Channel3` for reading and the corresponding write form, or a
dedicated built-in pair `channel_read(device, conn, channel)` /
`channel_write(device, conn, channel, val)`.
