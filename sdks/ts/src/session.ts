import type { TollboothClient } from "./client.js";
import type {
  MppSessionBearerCredential,
  MppSessionCloseCredential,
  MppSessionTopUpCredential,
  SessionCloseReceipt,
} from "./types.js";
import { handleMppCharge } from "./mpp.js";
import type { WalletLike } from "./types.js";

/** Convert a display amount string to raw units string (e.g. "1.5" with 6 decimals -> "1500000"). */
function displayToRaw(amount: string, decimals: number): string {
  const parts = amount.split(".");
  const whole = parts[0] || "0";
  const frac = (parts[1] || "").padEnd(decimals, "0").slice(0, decimals);
  return BigInt(whole + frac).toString();
}

/** Payment routing info stashed from the session challenge, needed for top-ups. */
export interface SessionPaymentInfo {
  recipient: string;
  mint: string;
  decimals: number;
  relay_url: string;
  fee_payer?: string;
}

/**
 * A session-based connection to a Tollbooth-protected API.
 *
 * Lifecycle: open (deposit) -> bearer requests -> optional top-up -> close (refund).
 */
export class TollboothSession {
  private sessionId: string | null = null;
  private bearer: string | null = null;
  private readonly client: TollboothClient;
  private readonly baseUrl: string;
  private paymentInfo: SessionPaymentInfo | null = null;
  private wallet: WalletLike | null = null;

  constructor(client: TollboothClient, baseUrl: string) {
    this.client = client;
    this.baseUrl = baseUrl.replace(/\/$/, "");
  }

  /**
   * Set session credentials after the open flow completes.
   * Called internally by TollboothClient.session().
   */
  setCredentials(
    sessionId: string,
    bearer: string,
    paymentInfo?: SessionPaymentInfo,
    wallet?: WalletLike,
  ): void {
    this.sessionId = sessionId;
    this.bearer = bearer;
    if (paymentInfo) this.paymentInfo = paymentInfo;
    if (wallet) this.wallet = wallet;
  }

  /**
   * Make an authenticated request using the session bearer token.
   */
  async fetch(path: string, init?: RequestInit): Promise<Response> {
    if (!this.sessionId || !this.bearer) {
      throw new Error("Session not opened, call client.session() first");
    }

    const url = `${this.baseUrl}${path}`;
    const credential: MppSessionBearerCredential = {
      type: "bearer",
      sessionId: this.sessionId,
      bearer: this.bearer,
    };

    const headers = new Headers(init?.headers);
    headers.set("X-Payment-Protocol", "mpp");
    headers.set("X-Payment-Credential", JSON.stringify(credential));

    return globalThis.fetch(url, {
      ...init,
      headers,
    });
  }

  /**
   * Top up an active session with additional funds.
   * Performs a transfer for the given amount and sends the topUp credential to the server.
   * Returns the top-up receipt from the server.
   */
  async topUp(amount: string): Promise<Record<string, unknown>> {
    if (!this.sessionId || !this.bearer) {
      throw new Error("Session not opened, call client.session() first");
    }
    if (!this.paymentInfo || !this.wallet) {
      throw new Error("Session missing payment info for top-up");
    }

    // Convert display amount to raw units for the prepare endpoint
    const rawAmount = displayToRaw(amount, this.paymentInfo.decimals);

    // Build and relay a transfer for the top-up amount
    // Note: top-ups do not include platform fees
    const signature = await handleMppCharge(
      {
        amount: rawAmount,
        ui_amount: amount,
        recipient: this.paymentInfo.recipient,
        mint: this.paymentInfo.mint,
        decimals: this.paymentInfo.decimals,
        relay_url: this.paymentInfo.relay_url,
        fee_payer: this.paymentInfo.fee_payer,
      },
      this.wallet,
    );

    // Send topUp credential to the server
    const credential: MppSessionTopUpCredential = {
      type: "topUp",
      sessionId: this.sessionId,
      signature,
    };

    const res = await globalThis.fetch(this.baseUrl, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Payment-Protocol": "mpp",
        "X-Payment-Credential": JSON.stringify(credential),
      },
      body: "{}",
    });

    if (!res.ok) {
      const text = await res.text();
      throw new Error(`Session top-up failed (${res.status}): ${text}`);
    }

    return res.json();
  }

  /**
   * Close the session and trigger a refund for unspent balance.
   */
  async close(): Promise<SessionCloseReceipt> {
    if (!this.sessionId || !this.bearer) {
      throw new Error("Session not opened, nothing to close");
    }

    const credential: MppSessionCloseCredential = {
      type: "close",
      sessionId: this.sessionId,
      bearer: this.bearer,
    };

    const url = this.baseUrl;
    const res = await globalThis.fetch(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Payment-Protocol": "mpp",
        "X-Payment-Credential": JSON.stringify(credential),
      },
      body: "{}",
    });

    if (!res.ok) {
      const text = await res.text();
      throw new Error(`Session close failed (${res.status}): ${text}`);
    }

    const receipt = (await res.json()) as SessionCloseReceipt;

    // Clear local state
    this.sessionId = null;
    this.bearer = null;

    return receipt;
  }

  /** Whether this session has been opened and not yet closed. */
  get isActive(): boolean {
    return this.sessionId !== null && this.bearer !== null;
  }

  /** The current session ID, or null if not active. */
  get id(): string | null {
    return this.sessionId;
  }
}
