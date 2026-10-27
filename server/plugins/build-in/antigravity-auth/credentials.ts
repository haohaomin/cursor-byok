import type { JsonValue, PluginContext } from "cursor-byok:plugin";
import type { ResourcePatch, ResourceSnapshot } from "cursor-byok:resource";
import { CLIENT_ID, CLIENT_SECRET, GOOGLE_TOKEN_URL } from "./google_oauth.ts";
import { accountData, isTokenExpired } from "./resources.ts";

function object(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function text(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function number(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim()) {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

/** Refreshes Google credentials before inference; the host persists this patch before retrying. */
export async function prepareAccount(
  resource: ResourceSnapshot,
  rejectedResource: ResourceSnapshot | null,
  context: PluginContext,
): Promise<ResourcePatch | null> {
  const data = accountData(resource);
  const rejected = rejectedResource !== null &&
    accountData(rejectedResource).accessToken === data.accessToken;
  if (!rejected && resource.state.status !== "invalid" && !isTokenExpired(data)) return null;
  if (!data.refreshToken) {
    return {
      state: { status: "invalid", message: "Google refresh token is missing; sign in again" },
    };
  }

  let response;
  try {
    response = await context.network.fetch(GOOGLE_TOKEN_URL, {
      method: "POST",
      headers: {
        accept: "application/json",
        "content-type": "application/x-www-form-urlencoded",
      },
      body: new URLSearchParams({
        client_id: CLIENT_ID,
        client_secret: CLIENT_SECRET,
        grant_type: "refresh_token",
        refresh_token: data.refreshToken,
      }).toString(),
    });
  } catch {
    return {
      state: {
        status: "cooling",
        retryAtMs: Date.now() + 15_000,
        message: "Google token refresh network error; retry shortly",
      },
    };
  }

  let body: Record<string, unknown> = {};
  try {
    body = object(JSON.parse(response.body)) ?? {};
  } catch {
    // Classified as a temporary response below.
  }
  if (response.status >= 200 && response.status < 300) {
    const accessToken = text(body.access_token);
    if (!accessToken) {
      return {
        state: {
          status: "cooling",
          retryAtMs: Date.now() + 15_000,
          message: "Google token refresh returned no access token; retry shortly",
        },
      };
    }
    const refreshToken = text(body.refresh_token) ?? data.refreshToken;
    const expiresIn = number(body.expires_in);
    const { expiresAtMs: _oldExpiry, ...previous } = data;
    return {
      privateData: {
        ...previous,
        accessToken,
        refreshToken,
        ...(expiresIn !== null && expiresIn > 0
          ? { expiresAtMs: Date.now() + expiresIn * 1000 }
          : {}),
      } as unknown as JsonValue,
      state: { status: "ready" },
    };
  }

  const errorCode = text(body.error);
  if (errorCode === "invalid_grant" || errorCode === "unauthorized_client") {
    return {
      state: {
        status: "invalid",
        message: "Google authorization expired or revoked; sign in again",
      },
    };
  }
  return {
    state: {
      status: "cooling",
      retryAtMs: Date.now() + 15_000,
      message: `Google token refresh temporarily failed (HTTP ${response.status}); retry shortly`,
    },
  };
}
