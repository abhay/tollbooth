import type { PublicKey, Transaction, VersionedTransaction } from "@solana/web3.js";

// ---------------------------------------------------------------------------
// Wallet abstraction
// ---------------------------------------------------------------------------

/**
 * Minimal wallet interface compatible with Phantom, wallet-adapter, and
 * Keypair-based signers.
 */
export interface WalletLike {
  publicKey: PublicKey | null;
  signTransaction<T extends Transaction | VersionedTransaction>(tx: T): Promise<T>;
  signMessage?(message: Uint8Array): Promise<Uint8Array | { signature: Uint8Array }>;
}

// ---------------------------------------------------------------------------
// Client configuration
// ---------------------------------------------------------------------------

export interface TollboothClientConfig {
  /** Wallet used for signing transactions and messages. */
  wallet: WalletLike;

  /**
   * Protocol preference. MPP is the only supported protocol.
   */
  protocol: "mpp";

  /** Solana network identifier (e.g. `'mainnet-beta'`, `'devnet'`). */
  network?: string;
}

// ---------------------------------------------------------------------------
// MPP protocol types
// ---------------------------------------------------------------------------

/** 402 challenge body for an MPP charge flow. */
export interface MppChallenge {
  /** Raw token amount (e.g. "1000000"). */
  amount: string;
  /** Display amount (e.g. "1.0"). */
  ui_amount: string;
  /** Recipient public key (base58). */
  recipient: string;
  /** SPL token mint (base58). */
  mint: string;
  /** Token decimal places. */
  decimals: number;
  /** Base URL for relay endpoints. */
  relay_url: string;
  /** Fee payer public key (the relayer, base58). */
  fee_payer?: string;
  /** Platform fee in raw units. */
  platform_fee?: string;
  /** Platform fee display amount. */
  platform_fee_ui_amount?: string;
  /** Platform fee recipient (base58). */
  platform_fee_recipient?: string;
}

/** 402 challenge body for an MPP session flow. */
export interface MppSessionChallenge {
  /** Deposit in raw units. */
  deposit: string;
  /** Deposit display amount. */
  deposit_ui_amount: string;
  /** Recipient public key (base58). */
  recipient: string;
  /** SPL token mint (base58). */
  mint: string;
  /** Token decimal places. */
  decimals: number;
  /** Base URL for relay endpoints. */
  relay_url: string;
  /** Fee payer public key (the relayer, base58). */
  fee_payer?: string;
  /** Platform fee in raw units. */
  platform_fee?: string;
  /** Platform fee display amount. */
  platform_fee_ui_amount?: string;
  /** Platform fee recipient (base58). */
  platform_fee_recipient?: string;
}

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

export type ProtocolKind = "mpp";

/** Receipt returned by the server after successful payment verification. */
export interface PaymentReceipt {
  protocol: ProtocolKind;
  signature: string;
  /** Amount in raw token units. */
  amount: string;
  /** Display amount. */
  uiAmount: string;
  payer: string;
  recipient: string;
  timestamp: number;
  /** Present for session operations. */
  sessionId?: string;
}

/** Relay endpoint response. */
export interface RelayResponse {
  signature: string;
}

// ---------------------------------------------------------------------------
// Session credential variants
// ---------------------------------------------------------------------------

export interface MppSessionOpenCredential {
  type: "open";
  signature: string;
  refundAddress: string;
  bearer: string;
}

export interface MppSessionBearerCredential {
  type: "bearer";
  sessionId: string;
  bearer: string;
}

export interface MppSessionTopUpCredential {
  type: "topUp";
  sessionId: string;
  signature: string;
}

export interface MppSessionCloseCredential {
  type: "close";
  sessionId: string;
  bearer: string;
}

export type MppSessionCredential =
  | MppSessionOpenCredential
  | MppSessionBearerCredential
  | MppSessionTopUpCredential
  | MppSessionCloseCredential;

/** Response from closing a session. */
export interface SessionCloseReceipt {
  sessionId: string;
  spent: string;
  refunded: string;
  refundSignature?: string;
}
