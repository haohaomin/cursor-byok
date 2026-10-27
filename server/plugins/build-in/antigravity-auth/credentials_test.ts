import type { JsonValue, PluginContext } from "cursor-byok:plugin";
import type { ResourceSnapshot } from "cursor-byok:resource";
import { prepareAccount } from "./credentials.ts";
import { CLIENT_ID, CLIENT_SECRET, GOOGLE_TOKEN_URL } from "./google_oauth.ts";
import { RESOURCE_TYPE } from "./resources.ts";

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function snapshot(
  privateData: JsonValue,
  state: ResourceSnapshot["state"] = { status: "ready" },
): ResourceSnapshot {
  return { id: "account-1", type: RESOURCE_TYPE, key: "antigravity:account-1", privateData, state };
}

Deno.test("Google refresh persists a rotated token and explicit expiry", async () => {
  let calls = 0;
  const context = {
    signal: new AbortController().signal,
    network: {
      fetch: async (url: string, init?: { headers?: Record<string, string>; body?: string }) => {
        calls += 1;
        assert(url === GOOGLE_TOKEN_URL, `unexpected token URL: ${url}`);
        assert(
          init?.headers?.["content-type"] === "application/x-www-form-urlencoded",
          "Google token endpoint requires form encoding",
        );
        const body = new URLSearchParams(init?.body);
        assert(body.get("client_id") === CLIENT_ID, "wrong Google client ID");
        assert(body.get("client_secret") === CLIENT_SECRET, "wrong Google client secret");
        assert(body.get("grant_type") === "refresh_token", "wrong grant type");
        assert(body.get("refresh_token") === "refresh-before", "wrong refresh token");
        return {
          status: 200,
          headers: {},
          body: JSON.stringify({
            access_token: "google-access-after",
            refresh_token: "refresh-after",
            expires_in: 3600,
          }),
        };
      },
      stream: () => Promise.reject(new Error("stream is not expected")),
    },
  } satisfies PluginContext;
  const patch = await prepareAccount(
    snapshot({
      accessToken: "google-access-before",
      refreshToken: "refresh-before",
      displayName: "test",
      projectId: "project",
      quota: null,
      expiresAtMs: Date.now() - 1,
    }),
    null,
    context,
  );
  assert(calls === 1, `expected one refresh request, got ${calls}`);
  const privateData = patch?.privateData as Record<string, JsonValue>;
  assert(patch?.state?.status === "ready", "refreshed account should be ready");
  assert(privateData.accessToken === "google-access-after", "new access token missing");
  assert(privateData.refreshToken === "refresh-after", "rotated refresh token missing");
  assert(
    typeof privateData.expiresAtMs === "number" && privateData.expiresAtMs > Date.now(),
    "expiry missing",
  );
});

Deno.test("Google invalid_grant requires a new login", async () => {
  const context = {
    signal: new AbortController().signal,
    network: {
      fetch: () =>
        Promise.resolve({
          status: 400,
          headers: {},
          body: JSON.stringify({ error: "invalid_grant" }),
        }),
      stream: () => Promise.reject(new Error("stream is not expected")),
    },
  } satisfies PluginContext;
  const patch = await prepareAccount(
    snapshot({
      accessToken: "google-access-before",
      refreshToken: "revoked-refresh",
      displayName: "test",
      quota: null,
      expiresAtMs: Date.now() - 1,
    }),
    null,
    context,
  );
  assert(patch?.state?.status === "invalid", "revoked Google token should require login");
});
