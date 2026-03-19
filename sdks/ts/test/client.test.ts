import { describe, expect, test } from "bun:test";
import { PublicKey } from "@solana/web3.js";
import { TollboothClient } from "../src/client.js";
import type {
  MppChallenge,
  WalletLike,
  TollboothClientConfig,
} from "../src/types.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Minimal mock wallet for unit tests (no actual signing). */
function mockWallet(): WalletLike {
  return {
    publicKey: new PublicKey("11111111111111111111111111111111"),
    async signTransaction(tx) {
      return tx;
    },
    async signMessage(_message: Uint8Array) {
      // Return a deterministic 64-byte "signature"
      return new Uint8Array(64).fill(42);
    },
  };
}

// ---------------------------------------------------------------------------
// TollboothClient constructor
// ---------------------------------------------------------------------------

describe("TollboothClient", () => {
  test("constructor accepts valid config", () => {
    const config: TollboothClientConfig = {
      wallet: mockWallet(),
      protocol: "mpp",
      network: "devnet",
    };

    const client = new TollboothClient(config);
    expect(client).toBeDefined();
    expect(client.config.protocol).toBe("mpp");
    expect(client.config.network).toBe("devnet");
  });

  test("constructor defaults to mainnet-beta when network is omitted", () => {
    const config: TollboothClientConfig = {
      wallet: mockWallet(),
      protocol: "mpp",
    };

    const client = new TollboothClient(config);
    expect(client).toBeDefined();
    expect(client.config.network).toBeUndefined();
    // The internal Connection uses mainnet-beta URL. We verify the client
    // was constructed without error.
  });

  test("constructor accepts mpp protocol", () => {
    const client = new TollboothClient({
      wallet: mockWallet(),
      protocol: "mpp",
    });
    expect(client.config.protocol).toBe("mpp");
  });
});

// ---------------------------------------------------------------------------
// MppChallenge type parsing
// ---------------------------------------------------------------------------

describe("MppChallenge", () => {
  test("parses a valid MPP charge challenge", () => {
    const raw = {
      amount: "0.001",
      recipient: "So1anaRecipientAddress1111111111111111111111",
      mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
      decimals: 6,
      relay_url: "https://api.example.com/relay",
      fee_payer: "KoraFeePayerAddress1111111111111111111111111",
    };

    // Parse as MppChallenge (simulates JSON.parse from a 402 body)
    const challenge: MppChallenge = raw;

    expect(challenge.amount).toBe("0.001");
    expect(challenge.recipient).toBe(
      "So1anaRecipientAddress1111111111111111111111",
    );
    expect(challenge.mint).toBe(
      "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    );
    expect(challenge.decimals).toBe(6);
    expect(challenge.relay_url).toBe("https://api.example.com/relay");
    expect(challenge.fee_payer).toBe(
      "KoraFeePayerAddress1111111111111111111111111",
    );
  });

  test("challenge fields have correct types", () => {
    const challenge: MppChallenge = {
      amount: "1.5",
      recipient: "11111111111111111111111111111111",
      mint: "11111111111111111111111111111111",
      decimals: 9,
      relay_url: "/relay",
    };

    expect(typeof challenge.amount).toBe("string");
    expect(typeof challenge.decimals).toBe("number");
    expect(typeof challenge.relay_url).toBe("string");
  });
});

// ---------------------------------------------------------------------------
// Bearer token generation (random, not derived from signature)
// ---------------------------------------------------------------------------

describe("bearer token generation", () => {
  test("randomUUID-based bearer produces a 64-char hex string", () => {
    const bearer =
      crypto.randomUUID().replace(/-/g, "") +
      crypto.randomUUID().replace(/-/g, "");
    expect(bearer).toMatch(/^[0-9a-f]{64}$/);
  });

  test("each bearer is unique", () => {
    const a =
      crypto.randomUUID().replace(/-/g, "") +
      crypto.randomUUID().replace(/-/g, "");
    const b =
      crypto.randomUUID().replace(/-/g, "") +
      crypto.randomUUID().replace(/-/g, "");
    expect(a).not.toBe(b);
  });
});

// ---------------------------------------------------------------------------
// TollboothSession
// ---------------------------------------------------------------------------

describe("TollboothSession", () => {
  test("session is not active before credentials are set", async () => {
    const { TollboothSession } = await import("../src/session.js");
    const client = new TollboothClient({
      wallet: mockWallet(),
      protocol: "mpp",
    });

    const session = new TollboothSession(client, "https://api.example.com");
    expect(session.isActive).toBe(false);
    expect(session.id).toBeNull();
  });

  test("session is active after credentials are set", async () => {
    const { TollboothSession } = await import("../src/session.js");
    const client = new TollboothClient({
      wallet: mockWallet(),
      protocol: "mpp",
    });

    const session = new TollboothSession(client, "https://api.example.com");
    session.setCredentials("session-123", "bearer-token-hex");
    expect(session.isActive).toBe(true);
    expect(session.id).toBe("session-123");
  });
});
