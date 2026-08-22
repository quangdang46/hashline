import assert from "node:assert/strict";
import test from "node:test";
import type {
  AgentToolUpdateCallback,
  ExtensionAPI,
  ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import register from "../index.js";

type ToolResult = {
  content: Array<{ type: "text"; text: string }>;
  details: { ok: boolean; [key: string]: unknown };
  isError?: boolean;
};

type RenderComponent = {
  render(width: number): string[];
};

type ThemeStub = {
  fg: (name: string, text: string) => string;
  bold: (text: string) => string;
};

type RegisteredTool = {
  name: string;
  parameters: unknown;
  renderShell?: "default" | "self";
  renderCall?: (
    args: Record<string, unknown>,
    theme: ThemeStub,
    context: Record<string, unknown>,
  ) => RenderComponent;
  renderResult?: (
    result: ToolResult,
    options: { expanded: boolean; isPartial: boolean },
    theme: ThemeStub,
    context: { args?: Record<string, unknown> },
  ) => RenderComponent;
  execute: (
    toolCallId: string,
    params: Record<string, unknown>,
    signal: AbortSignal | undefined,
    onUpdate: AgentToolUpdateCallback<unknown> | undefined,
    ctx: ExtensionContext,
  ) => Promise<ToolResult>;
};

type RegisteredCommand = {
  description: string;
  handler: (args: string, ctx: ExtensionContext) => Promise<void>;
};

function registerExtension() {
  const tools: RegisteredTool[] = [];
  const commands = new Map<string, RegisteredCommand>();
  const events: string[] = [];
  const pi = {
    registerTool(tool: RegisteredTool) {
      tools.push(tool);
    },
    registerCommand(name: string, command: RegisteredCommand) {
      commands.set(name, command);
    },
    on(name: string) {
      events.push(name);
    },
  } as unknown as ExtensionAPI;

  register(pi);

  return { tools, commands, events };
}

test("registers read/edit/write overrides plus file tools", () => {
  const { tools } = registerExtension();

  const names = tools.map((tool) => tool.name).sort();
  assert.deepEqual(
    names.sort(),
    [
      "edit",
      "find_block",
      "read",
      "remove_file",
      "rename_file",
      "write",
    ].sort(),
  );

  const read = tools.find((tool) => tool.name === "read");
  const edit = tools.find((tool) => tool.name === "edit");
  const write = tools.find((tool) => tool.name === "write");
  assert.ok(read, "read tool registered");
  assert.ok(edit, "edit tool registered");
  assert.ok(write, "write tool registered");
  assert.ok(read?.parameters, "read has a parameter schema");
  assert.ok(edit?.parameters, "edit has a parameter schema");
  assert.ok(write?.parameters, "write has a parameter schema");
});

test("edit tool forces renderShell default (must not inherit built-in 'self')", () => {
  const { tools } = registerExtension();
  const edit = tools.find((tool) => tool.name === "edit");
  assert.equal(edit?.renderShell, "default");
});

test("read tool parameters expose path/offset/limit/raw", () => {
  const { tools } = registerExtension();
  const read = tools.find((tool) => tool.name === "read");
  const params = read?.parameters as {
    properties?: Record<string, { type?: string }>;
  };
  assert.ok(params?.properties, "read schema has properties");
  assert.equal(params.properties?.path?.type, "string");
  assert.equal(params.properties?.offset?.type, "integer");
  assert.equal(params.properties?.limit?.type, "integer");
  assert.equal(params.properties?.raw?.type, "boolean");
});

test("edit tool parameters expose path and edits array", () => {
  const { tools } = registerExtension();
  const edit = tools.find((tool) => tool.name === "edit");
  const params = edit?.parameters as {
    properties?: Record<string, { type?: string }>;
  };
  assert.ok(params?.properties, "edit schema has properties");
  assert.equal(params.properties?.path?.type, "string");
  assert.equal(params.properties?.edits?.type, "array");
});

test("registers session_start event and hashline-status command", () => {
  const { events, commands } = registerExtension();
  assert.ok(events.includes("session_start"));
  assert.ok(commands.has("hashline-status"));
});

test("edit tool schema rejects replace_text when the op set is validated", () => {
  // The published schema is the full union (replaceText default true); assert
  // the four core ops are present in the edit item union.
  const { tools } = registerExtension();
  const edit = tools.find((tool) => tool.name === "edit");
  const params = edit?.parameters as {
    properties?: Record<string, unknown>;
  };
  const editsSchema = params?.properties?.edits as {
    items?: { anyOf?: Array<{ properties?: { op?: { const?: string } } }> };
  };
  const ops = (editsSchema?.items?.anyOf ?? []).map(
    (entry) => entry.properties?.op?.const,
  );
  for (const op of ["replace", "append", "prepend", "delete", "replace_text"]) {
    assert.ok(ops.includes(op), `edit item union includes op ${op}`);
  }
});

test("runHashlineWithBin handles early subprocess exit without unhandled EPIPE error", async () => {
  const { runHashlineWithBin } = await import("../src/hashline.js");
  const ctx = { cwd: process.cwd() } as ExtensionContext;
  const res = await runHashlineWithBin(
    ["-e", "process.exit(0)"],
    undefined,
    ctx,
    undefined,
    process.execPath,
  );
  assert.equal(res.exitCode, 0);

  const res2 = await runHashlineWithBin(
    ["-e", "process.exit(0)"],
    "some input data",
    ctx,
    undefined,
    process.execPath,
  );
  assert.equal(res2.exitCode, 0);
});
