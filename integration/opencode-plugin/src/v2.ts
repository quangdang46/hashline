/**
 * v2.ts — OpenCode 2 (beta plugin API) entrypoint.
 *
 * opencode2 requires the default export to be `Plugin.define({ id, setup })`
 * and custom tools to be added via `ctx.tool.transform((tools) => tools.add(...))`
 * with JSON-Schema inputs. The v1 `{ tool: {...} }` hooks shape is silently
 * ignored there, so this module bridges the two:
 *
 *   - invokes the shared v1 `plugin()` factory once with a minimal context,
 *   - registers every returned hook tool as a native v2 tool (zod args schemas
 *     satisfy the Standard-JSON-Schema interface Tool.Info.input accepts),
 *   - injects the hashline system prompt through ctx.session.hook("context").
 */

import { plugin } from "./plugin";
import { renderHashlineEditPrompt } from "./prompt";

type V1Tool = {
  description: string;
  args: unknown; // zod schema — satisfies Standard JSON-Schema for v2
  execute: (
    args: unknown,
    context: { abort?: AbortSignal; metadata: (m: unknown) => void },
  ) => Promise<string>;
};

type V1Hooks = {
  tool: Record<string, V1Tool>;
  "experimental.chat.system.transform": (
    input: unknown,
    output: { system: unknown[] },
  ) => Promise<void>;
};

// Runtime uses opencode2's own bundled beta SDK where `Plugin.define` exists;
// typed here as a plain object because the repo pins the 1.x types for plugin.ts.
const v2Plugin = {
  id: "hashline-opencode-plugin",
  setup: async (ctx: {
    tool: {
      transform: (
        cb: (tools: {
          add(tool: {
            name: string;
            description: string;
            input: unknown;
            execute: (
              input: unknown,
            ) => Promise<{ content?: string }>;
          }): void;
        }) => void,
      ) => Promise<unknown>;
    };
    session: {
      hook(
        event: "context",
        callback: (input: { system: unknown[] }) => Promise<void>,
      ): Promise<unknown>;
    };
  }) => {
    const hooks = (await (
      plugin as unknown as (ctx: unknown) => Promise<V1Hooks>
    )({
      directory: process.cwd(),
      worktree: process.cwd(),
      client: {} as never,
    })) as V1Hooks;

    // System prompt: v2 validates each entry as an LLM.SystemPart object and
    // rejects raw strings. Render the guidance once and append it as a proper
    // part-shaped object instead of delegating to the v1 string transform.
    const rendered = renderHashlineEditPrompt();
    await ctx.session.hook("context", async (event) => {
      // Defensive: drop any raw-string entries other loaders may have pushed
      // (v2 validates every entry as an LLM.SystemPart object).
      const system = event.system as unknown[];
      for (let i = system.length - 1; i >= 0; i--) {
        if (typeof system[i] === "string") system.splice(i, 1);
      }
      system.push({ type: "text", text: rendered });
    });

    // Tools: re-register each v1 hook tool under the same name.
    // The v1 tool bodies resolve paths via context.directory/worktree — the
    // plugin-level baseDir captured at factory time. Forward the live ToolContext
    // (id/sessionID/agent/messageID per opencode2 docs) plus those fields.
    const baseDir = process.cwd();
    for (const [name, def] of Object.entries(hooks.tool)) {
      const execute = def.execute;
      await ctx.tool.transform((tools) => {
        tools.add({
          name,
          description: def.description,
          input: def.args as never,
          execute: async (input: unknown, toolCtx: unknown) => ({
            content: await execute(input, {
              ...((toolCtx ?? {}) as Record<string, unknown>),
              abort: undefined,
              directory: baseDir,
              worktree: baseDir,
              metadata: () => {},
            } as never),
          }),
        } as never);
      });
    }
  },
};

export default v2Plugin;
