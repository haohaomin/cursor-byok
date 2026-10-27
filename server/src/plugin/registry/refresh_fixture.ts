import { defineProviderPlugin } from "cursor-byok:plugin";

type TestData = { token: string; delayMs?: number; rejectedDelayMs?: number };

// Synthetic worker state is only observed by the host integration tests.
let exchanges = 0;
const invocations: unknown[] = [];

export default defineProviderPlugin({
  providers: [{
    id: "fake", displayName: "fake", providerType: "test", resourceType: "account",
    invoke: async (input, output) => {
      invocations.push(structuredClone(input));
      const scenario = input.request.instructions;
      if (scenario === "after-output") {
        output.emit({ type: "text-delta", text: "partial" });
      }
      if (scenario === "always-reject" || scenario === "after-output" ||
          (scenario === "retry" && invocations.length === 1)) {
        return { status: "auth-error", message: "synthetic 401" };
      }
      output.emit({ type: "text-delta", text: (input.resource!.privateData as TestData).token });
      output.emit({ type: "done", reason: "stop" });
      return { status: "completed" };
    },
  }],
  resources: [{
    type: "account", displayName: "account",
    present: () => ({ displayName: JSON.stringify({ exchanges, invocations }) }),
    prepare: async (resource, rejected) => {
      const data = resource.privateData as TestData;
      if (data.token.startsWith("fresh-") && !rejected) return null;
      const generation = ++exchanges;
      await new Promise(resolve => setTimeout(resolve,
        rejected ? (data.rejectedDelayMs ?? 150) : (data.delayMs ?? 150)));
      return {
        privateData: { ...data, token: `fresh-${generation}`, refreshToken: `rotated-${generation}` },
        state: { status: "ready" },
      };
    },
  }],
});
