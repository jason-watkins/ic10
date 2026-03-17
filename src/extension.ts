import * as child_process from "child_process";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel | undefined;

export async function activate(context: vscode.ExtensionContext) {
    outputChannel = vscode.window.createOutputChannel("IC20");
    context.subscriptions.push(outputChannel);

    context.subscriptions.push(
        vscode.languages.registerDocumentFormattingEditProvider("ic20", {
            provideDocumentFormattingEdits(document, options) {
                return formatDocument(document, options);
            },
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand("ic20.build", () => buildCurrentFile(context))
    );

    const config = vscode.workspace.getConfiguration("ic20.lsp");
    const configuredPath = config.get<string>("path", "");

    let command: string;
    if (configuredPath) {
        command = configuredPath;
    } else {
        const ext = process.platform === "win32" ? ".exe" : "";
        const bundledName = `ic20-lsp-${process.platform}-${process.arch}${ext}`;
        const bundledPath = path.join(context.extensionPath, "bin", bundledName);
        command = fs.existsSync(bundledPath) ? bundledPath : "ic20-lsp";
    }

    const serverOptions: ServerOptions = {
        command,
        args: [],
    };

    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: "file", language: "ic20" }],
    };

    client = new LanguageClient(
        "ic20-lsp",
        "IC20 Language Server",
        serverOptions,
        clientOptions
    );

    try {
        await client.start();
    } catch {
        client = undefined;
        vscode.window.showWarningMessage(
            `Could not start IC20 language server ('${command}'). ` +
            "Diagnostics, hover, and go-to-definition will be unavailable. " +
            "Set ic20.lsp.path in settings or add ic20-lsp to your PATH."
        );
    }
}

async function buildCurrentFile(context: vscode.ExtensionContext): Promise<void> {
    const document = vscode.window.activeTextEditor?.document;
    if (!document || document.languageId !== "ic20") {
        vscode.window.showErrorMessage("No IC20 file is active.");
        return;
    }

    if (document.isDirty) {
        await document.save();
    }

    const sourceUri = document.uri;
    if (sourceUri.scheme !== "file") {
        vscode.window.showErrorMessage("IC20 build requires a file saved to disk.");
        return;
    }

    const sourcePath = sourceUri.fsPath;
    const outputPath = sourcePath.replace(/\.ic20$/, ".ic10");

    const compilerPath = resolveCompilerPath(context);

    outputChannel!.clear();
    outputChannel!.show(true);
    outputChannel!.appendLine(`Building ${path.basename(sourcePath)}...`);
    outputChannel!.appendLine(`  ${compilerPath} "${sourcePath}" -o "${outputPath}"`);

    await new Promise<void>((resolve) => {
        const proc = child_process.spawn(compilerPath, [sourcePath, "-o", outputPath], {
            stdio: ["ignore", "pipe", "pipe"],
        });

        proc.stdout.on("data", (data: Buffer) => {
            outputChannel!.append(data.toString());
        });

        proc.stderr.on("data", (data: Buffer) => {
            outputChannel!.append(data.toString());
        });

        proc.on("close", (code) => {
            if (code === 0) {
                outputChannel!.appendLine(`\nBuild succeeded: ${path.basename(outputPath)}`);
                vscode.window.showInformationMessage(
                    `IC20 build succeeded: ${path.basename(outputPath)}`
                );
            } else {
                outputChannel!.appendLine(`\nBuild failed (exit code ${code}).`);
                vscode.window.showErrorMessage(
                    `IC20 build failed. See the IC20 output channel for details.`
                );
            }
            resolve();
        });

        proc.on("error", (err) => {
            outputChannel!.appendLine(`\nFailed to launch compiler: ${err.message}`);
            outputChannel!.appendLine(
                `Ensure ic20c is on your PATH or set ic20.compiler.path in settings.`
            );
            vscode.window.showErrorMessage(
                `Could not launch IC20 compiler ('${compilerPath}'). ` +
                "Set ic20.compiler.path in settings or add ic20c to your PATH."
            );
            resolve();
        });
    });
}

function resolveCompilerPath(context: vscode.ExtensionContext): string {
    const config = vscode.workspace.getConfiguration("ic20.compiler");
    const configuredPath = config.get<string>("path", "");
    if (configuredPath) {
        return configuredPath;
    }
    const ext = process.platform === "win32" ? ".exe" : "";
    const bundledName = `ic20c-${process.platform}-${process.arch}${ext}`;
    const bundledPath = path.join(context.extensionPath, "bin", bundledName);
    return fs.existsSync(bundledPath) ? bundledPath : "ic20c";
}

export async function deactivate() {
    if (client) {
        await client.stop();
    }
}

function formatDocument(
    document: vscode.TextDocument,
    options: vscode.FormattingOptions
): vscode.TextEdit[] {
    const source = document.getText();
    const indent = options.insertSpaces ? " ".repeat(options.tabSize) : "\t";
    const segments = reflowSource(source);
    const lines = assignIndentation(segments, indent);
    const newText = lines.join("\n") + "\n";
    const fullRange = new vscode.Range(
        document.positionAt(0),
        document.positionAt(source.length)
    );
    if (source === newText) return [];
    return [vscode.TextEdit.replace(fullRange, newText)];
}

const LINE_WIDTH = 100;

function reflowSource(source: string): string[] {
    const segments: string[] = [];
    let current = "";
    let inString = false;
    let inLineComment = false;
    let inlineComment = false;
    let emptyLinesSince = 0;
    let hadNewlineSinceLastPush = true;

    function pushRaw(seg: string) {
        if (emptyLinesSince >= 2 && segments.length > 0) {
            segments.push("");
        }
        segments.push(seg);
        emptyLinesSince = 0;
        hadNewlineSinceLastPush = false;
    }

    function pushSegment(content: string) {
        const trimmed = content.trim();
        if (trimmed) pushRaw(trimmed);
    }

    for (let i = 0; i < source.length; i++) {
        const ch = source[i];

        if (ch === "\r") continue;

        if (ch === "\n") {
            if (inLineComment) {
                inLineComment = false;
                const commentText = current.trim();
                if (inlineComment && segments.length > 0) {
                    segments[segments.length - 1] += "  " + commentText;
                } else if (commentText) {
                    pushRaw(commentText);
                }
                inlineComment = false;
                current = "";
                emptyLinesSince = 1;
            } else if (current.trim() === "") {
                emptyLinesSince++;
                current = "";
            }
            hadNewlineSinceLastPush = true;
            continue;
        }

        if (inLineComment) {
            current += ch;
            continue;
        }

        if (!inString && ch === "/" && i + 1 < source.length && source[i + 1] === "/") {
            const hasContentBefore = current.trim() !== "";
            if (hasContentBefore) {
                pushSegment(current);
                current = "";
            }
            inlineComment = !hadNewlineSinceLastPush && !hasContentBefore && segments.length > 0;
            current = "//";
            inLineComment = true;
            i++;
            continue;
        }

        if (ch === '"') {
            inString = !inString;
            current += ch;
            continue;
        }

        if (inString) {
            current += ch;
            continue;
        }

        if (ch === "{") {
            current += ch;
            pushSegment(current);
            current = "";
        } else if (ch === "}") {
            pushSegment(current);
            current = "";
            pushRaw("}");
        } else if (ch === ";") {
            current += ";";
            pushSegment(current);
            current = "";
        } else {
            current += ch;
        }
    }

    if (inLineComment) {
        const commentText = current.trim();
        if (inlineComment && segments.length > 0) {
            segments[segments.length - 1] += "  " + commentText;
        } else if (commentText) {
            pushRaw(commentText);
        }
    } else {
        pushSegment(current);
    }

    return segments;
}

function findCommentStart(seg: string): number {
    let inStr = false;
    for (let i = 0; i < seg.length - 1; i++) {
        if (seg[i] === '"') inStr = !inStr;
        if (!inStr && seg[i] === "/" && seg[i + 1] === "/") return i;
    }
    return -1;
}

function wrapCommentSegment(seg: string, indentation: string): string[] {
    const fullLine = indentation + seg;
    if (fullLine.length <= LINE_WIDTH) return [fullLine];

    const match = seg.match(/^(\/\/\s*)/);
    const commentPrefix = match ? match[1] : "// ";
    const text = seg.slice(commentPrefix.length);
    const words = text.split(/\s+/).filter(w => w.length > 0);

    if (words.length === 0) return [indentation + "//"];

    const linePrefix = indentation + "//";
    const lines: string[] = [];
    let line = linePrefix;

    for (const word of words) {
        const candidate = line === linePrefix ? linePrefix + " " + word : line + " " + word;
        if (candidate.length > LINE_WIDTH && line !== linePrefix) {
            lines.push(line);
            line = linePrefix + " " + word;
        } else {
            line = candidate;
        }
    }
    if (line !== linePrefix) {
        lines.push(line);
    }

    return lines.length > 0 ? lines : [fullLine];
}

function assignIndentation(segments: string[], indent: string): string[] {
    let depth = 0;
    const lines: string[] = [];

    for (const seg of segments) {
        if (seg === "") {
            lines.push("");
            continue;
        }

        const commentIdx = findCommentStart(seg);
        const codePart = commentIdx === -1 ? seg : seg.slice(0, commentIdx).trim();

        if (codePart.startsWith("}")) {
            depth = Math.max(0, depth - 1);
        }

        const indentation = indent.repeat(depth);
        if (seg.startsWith("//")) {
            for (const wrapped of wrapCommentSegment(seg, indentation)) {
                lines.push(wrapped);
            }
        } else {
            lines.push(indentation + seg);
        }

        if (codePart.endsWith("{")) {
            depth++;
        }
    }

    return lines;
}
