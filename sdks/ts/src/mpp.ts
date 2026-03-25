import { VersionedTransaction } from "@solana/web3.js";
import type {
  MppChallenge,
  MppSessionChallenge,
  RelayResponse,
  WalletLike,
} from "./types.js";

/**
 * Handle an MPP charge flow (server-first signing):
 *
 * 1. POST to /relay/prepare — server builds and fee-payer-signs the tx.
 * 2. Counter-sign with wallet (simulation succeeds, fee payer is present).
 * 3. POST fully-signed tx to /relay — server validates and submits.
 * 4. Return the confirmed transaction signature.
 */
export async function handleMppCharge(
  challenge: MppChallenge,
  wallet: WalletLike,
): Promise<string> {
  if (!challenge.relay_url) {
    throw new Error("relay_url is required for gasless MPP flow");
  }
  if (!wallet.publicKey) {
    throw new Error("Wallet not connected");
  }

  // Build prepare request — with splits if platform fee is present
  const body = challenge.platform_fee && challenge.platform_fee_recipient
    ? {
        payer: wallet.publicKey.toBase58(),
        splits: [
          { amount: challenge.amount, recipient: challenge.recipient },
          { amount: challenge.platform_fee, recipient: challenge.platform_fee_recipient },
        ],
      }
    : {
        payer: wallet.publicKey.toBase58(),
        amount: challenge.amount,
      };

  // 1. Request a pre-signed transaction from the server
  const prepareRes = await fetch(`${challenge.relay_url}/prepare`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!prepareRes.ok) {
    const text = await prepareRes.text();
    throw new Error(`Prepare request failed (${prepareRes.status}): ${text}`);
  }

  // 2. Deserialize the partially-signed transaction
  const txBytes = new Uint8Array(await prepareRes.arrayBuffer());
  const tx = VersionedTransaction.deserialize(txBytes);

  // 3. Counter-sign as token authority (wallet simulation succeeds)
  const signed = await wallet.signTransaction(tx);

  // 4. Submit the fully-signed transaction via relay
  const relayRes = await fetch(challenge.relay_url, {
    method: "POST",
    headers: { "Content-Type": "application/octet-stream" },
    body: new Uint8Array(signed.serialize()) as unknown as BodyInit,
  });
  if (!relayRes.ok) {
    const text = await relayRes.text();
    throw new Error(`Relay request failed (${relayRes.status}): ${text}`);
  }

  const { signature } = (await relayRes.json()) as RelayResponse;
  return signature;
}

/**
 * Build and relay an MPP session deposit transaction.
 * Returns the deposit signature.
 */
export async function handleMppSessionDeposit(
  challenge: MppSessionChallenge,
  wallet: WalletLike,
): Promise<string> {
  return handleMppCharge(
    { ...challenge, amount: challenge.deposit, ui_amount: challenge.deposit_ui_amount },
    wallet,
  );
}

