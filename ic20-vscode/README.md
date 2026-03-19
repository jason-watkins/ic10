# IC20 Language

Language support for **IC20**, a Rust-inspired language that compiles to IC10 assembly for [Stationeers](https://store.steampowered.com/app/544550/Stationeers/) programmable chips.

## Features

- **Syntax highlighting** for IC20 (`.ic20`) and IC10 (`.ic10`) files
- **Diagnostics, hover, and go-to-definition** via bundled language servers (ic20-lsp and ic10-lsp)
- **Build command** — compile IC20 to IC10 assembly directly from the editor (`Ctrl+Shift+B` or right-click → Build IC20 File)
- **Document formatting** for IC20 files
- **Snippets** for common IC20 constructs (`fn`, `device`, `let`, `if`, `loop`, etc.)

## Quick Start

1. Install the extension.
2. Open or create a `.ic20` file.
3. Start writing IC20 code — you'll get syntax highlighting, diagnostics, and completions out of the box.
4. Press `Ctrl+Shift+B` to compile to IC10 assembly. The output file is written next to the source (e.g. `main.ic20` → `main.ic10`).
5. Paste the IC10 output into a programmable chip in Stationeers.

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

This reads a temperature sensor every game tick and toggles a heater on or off.

## Configuration

| Setting | Description | Default |
| --- | --- | --- |
| `ic20.lsp.path` | Path to the ic20-lsp binary | Bundled binary, then `PATH` |
| `ic20.compiler.path` | Path to the ic20c compiler binary | Bundled binary, then `PATH` |
| `ic10.lsp.path` | Path to the ic10-lsp binary | Bundled binary, then `PATH` |

If the bundled binary for your platform isn't available, the extension falls back to searching your `PATH`. You can also point each setting at a specific binary.

## Building from Source

The extension bundles platform-specific binaries for the language servers and compiler. To build everything yourself:

```sh
# Build the Rust binaries
cargo build --release --manifest-path ic20-lsp/Cargo.toml
cargo build --release --manifest-path ic10-lsp/Cargo.toml
cargo build --release --manifest-path ic20c/Cargo.toml

# Bundle the extension
cd ic20-vscode
npm install
npm run package
```

This produces a `.vsix` file you can install with `code --install-extension <file>.vsix`.

## Links

- [IC20 language specification](https://github.com/jason-watkins/ic10/blob/main/ic20c/ic20.md)
- [Compiler documentation](https://github.com/jason-watkins/ic10/blob/main/ic20c/README.md)
- [Source code](https://github.com/jason-watkins/ic10)
- [Issue tracker](https://github.com/jason-watkins/ic10/issues)

## License

[MIT](LICENSE)
