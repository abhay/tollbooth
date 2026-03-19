import type {
  MppChallenge,
  MppSessionChallenge,
  TollboothClientConfig,
} from "./types.js";
import { handleMppCharge, handleMppSessionDeposit } from "./mpp.js";
import { TollboothSession } from "./session.js";

/**
 * TollboothClient: a fetch-like interface that automatically handles 402
 * payment challenges from Tollbooth-protected APIs.
 *
 * Uses MPP (relay-based, gasless) protocol.
 */
export class TollboothClient {
  readonly config: TollboothClientConfig;

  constructor(config: TollboothClientConfig) {
    this.config = config;
  }

  /**
   * Make a request to a Tollbooth-protected endpoint.
   *
   * If the server responds with 402 Payment Required, the client
   * automatically handles the payment flow based on the configured
   * protocol and retries the request with the payment credential.
   */
  async fetch(url: string, init?: RequestInit): Promise<Response> {
    // Build headers, preserving any user-supplied headers
    const headers = new Headers(init?.headers);
    headers.set("X-Payment-Protocol", "mpp");

    // Initial request
    const res = await globalThis.fetch(url, { ...init, headers });

    if (res.status !== 402) {
      return res;
    }

    // 402 Payment Required: handle the MPP challenge
    const body = await res.json();
    const credential = await this.handleMpp(body as MppChallenge);

    // Retry with payment credential
    const retryHeaders = new Headers(init?.headers);
    retryHeaders.set("X-Payment-Protocol", "mpp");
    retryHeaders.set("X-Payment-Signature", credential);

    return globalThis.fetch(url, { ...init, headers: retryHeaders });
  }

  /**
   * Open a session-based connection to a Tollbooth-protected API.
   *
   * Performs the deposit flow and returns a TollboothSession that can be
   * used for repeated requests using the bearer token.
   */
  async session(baseUrl: string): Promise<TollboothSession> {
    const session = new TollboothSession(this, baseUrl);

    // Request the session challenge
    const headers = new Headers();
    headers.set("X-Payment-Protocol", "mpp");

    const res = await globalThis.fetch(baseUrl, { headers });

    if (res.status !== 402) {
      throw new Error(
        `Expected 402 session challenge, got ${res.status}`,
      );
    }

    const challenge = (await res.json()) as MppSessionChallenge;

    // Perform the deposit
    const depositSignature = await handleMppSessionDeposit(
      challenge,
      this.config.wallet,
    );

    // Generate a random bearer secret (64 hex chars)
    const bearer = crypto.randomUUID().replace(/-/g, '') + crypto.randomUUID().replace(/-/g, '');

    // Open the session with the server (bearer is sent in the credential)
    const openCredential = JSON.stringify({
      type: "open",
      signature: depositSignature,
      refundAddress: this.config.wallet.publicKey!.toBase58(),
      bearer,
    });

    const openHeaders = new Headers();
    openHeaders.set("X-Payment-Protocol", "mpp");
    openHeaders.set("X-Payment-Credential", openCredential);

    const openRes = await globalThis.fetch(baseUrl, { headers: openHeaders });

    if (!openRes.ok) {
      const text = await openRes.text();
      throw new Error(`Session open failed (${openRes.status}): ${text}`);
    }

    const receipt = await openRes.json();
    if (!receipt.sessionId) {
      throw new Error("Server returned session open receipt without sessionId");
    }
    session.setCredentials(receipt.sessionId, bearer, {
      recipient: challenge.recipient,
      mint: challenge.mint,
      decimals: challenge.decimals,
      relay_url: challenge.relay_url,
      fee_payer: challenge.fee_payer,
    }, this.config.wallet);

    return session;
  }

  // -----------------------------------------------------------------------
  // Private helpers
  // -----------------------------------------------------------------------

  private async handleMpp(challenge: MppChallenge): Promise<string> {
    return handleMppCharge(
      challenge,
      this.config.wallet,
    );
  }
}
