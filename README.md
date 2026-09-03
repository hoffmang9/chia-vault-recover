# Chia Vault Recover

Recover a [Chia Cloud Wallet](https://www.chia.net/) vault using the BIP39 recovery passphrase, then rekey custody to a new BLS mnemonic.

Licensed under the [Apache License 2.0](LICENSE).

## What you need

This tool **requires** a Cloud Wallet **vault configuration file** (`vault-config-*.json`). A vault address alone is not enough.

You must have:

1. **Vault config JSON** — public vault layout: launcher id, custody and recovery public keys, thresholds, and clawback timelock. See [Download your vault config](#download-your-vault-config). Keep a copy with your backups; it does not contain private keys or your recovery phrase.
2. **Recovery passphrase** — the 24-word phrase Cloud Wallet gave you for vault recovery (signs delayed recovery).
3. **A new custody mnemonic** — 24 words by default (12 optional); this becomes the post-recovery spend key. The tool can auto-generate a second mnemonic for the new recovery branch.

You also need network access (coinset by default, or a full node) to find the vault singleton and broadcast transactions.

## What it does

Cloud Wallet vaults are MIPS 1-of-2 singletons (custody | recovery). This tool runs **delayed (timelocked) recovery**:

1. **inspect** — match your `vault-config-*.json` to the on-chain singleton
2. **start** — sign with the recovery phrase; vault enters RECOVERY (custody can still claw back with the old passkey)
3. wait for `clawbackTimelock` seconds (often 43200 / 12h)
4. **finish** — permissionless rekey to the new custody configuration

Default destination: new **24-word** BLS custody + an **auto-generated** second BLS recovery mnemonic (12-word option available). The generated recovery mnemonic is shown once (CLI print / GUI clipboard) and is **not** written into the public post-recovery config file.

## Download your vault config

A **Download Config** button is coming soon to [vault.chia.net](https://vault.chia.net). Until then, export the file from a logged-in Cloud Wallet tab.

The file is public vault layout only. It does **not** include your recovery passphrase or any private key.

1. Log in at [vault.chia.net](https://vault.chia.net) (or the testnet Cloud Wallet host).
2. Open the vault you want to export.
3. Open DevTools → Console (macOS: Option-Command-J; Windows/Linux: Ctrl+Shift+J).
4. Paste the contents of [`scripts/download-vault-config.js`](scripts/download-vault-config.js) and press Enter.
5. The browser downloads one `vault-config-*.json`. Store it with your backups.
6. Confirm it matches the chain before recovery:

```bash
chia-vault-recover inspect --config vault-config.json
```

Only paste that script on the Cloud Wallet site while you are logged in. It uses your existing session to read vault public keys from the same GraphQL API the page already calls.

## Install / build

```bash
cargo build --release
# binaries: target/release/chia-vault-recover
#           target/release/chia-vault-recover-gui
```

## CLI

```bash
# Verify config vs chain (mainnet + coinset by default)
chia-vault-recover inspect --config vault-config.json

# Start delayed recovery
chia-vault-recover start \
  --config vault-config.json \
  --recovery-mnemonic-file recovery.txt \
  --new-custody-mnemonic-file new-custody.txt \
  --out-config post-recovery-vault-config.json

# After the clawback window
chia-vault-recover finish \
  --config vault-config.json \
  --post-recovery-config post-recovery-vault-config.json
```

Useful flags:

| Flag | Meaning |
|------|---------|
| `--network mainnet\|testnet11` | Default **mainnet** |
| `--backend coinset\|rpc` | Default **coinset**; with `rpc` set `--full-node-url` |
| `--word-count 12\|24` | Length for auto-generated recovery mnemonic (default 24) |

Fees are not supported yet (zero-fee spends only).

Mnemonics may also be passed via env: `CHIA_VAULT_RECOVERY_MNEMONIC`, `CHIA_VAULT_NEW_CUSTODY_MNEMONIC`.

## GUI

```bash
chia-vault-recover-gui
```

Load the vault config file, paste mnemonics, then Inspect / Start / Finish. After Start, copy the generated recovery mnemonic with the clipboard button.

## Key derivation (Cloud Wallet compatible)

```
BIP39 mnemonic → seed("") → AugSchemeMPL.keyGen / SecretKey::from_seed
```

No `m/12381/8444/...` path. Matches Cloud Wallet `bls.ts`.

## Testing

CI runs **Simulator end-to-end** recovery tests (real puzzles and signatures via `chia-sdk-test`). No live-network CI.

```bash
cargo test --all
```

### Manual testnet11 recipe

1. Use a saved `vault-config-*.json` for a testnet vault (see [Download your vault config](#download-your-vault-config)).
2. Ensure the vault singleton is unspent (zero-fee spends only).
3. Run against testnet:

```bash
chia-vault-recover inspect --config vault-config.json --network testnet11
chia-vault-recover start --config vault-config.json --network testnet11 \
  --recovery-mnemonic-file recovery.txt \
  --new-custody-mnemonic-file new-custody.txt
# wait clawbackTimelock
chia-vault-recover finish --config vault-config.json --network testnet11 \
  --post-recovery-config post-recovery-vault-config.json
```

Or point `--backend rpc --full-node-url https://localhost:8555` at a synced full node.

## Caveats

- Instant recovery (spend/passkey key) is out of scope — use delayed recovery with the passphrase.
- Clawback during the window still requires the old custody passkey (not implemented here).
- After finish, custody is on-chain BLS; Cloud Wallet’s product UI may not re-import that vault as a normal passkey vault.
- p2-singleton XCH/CATs are unchanged; only the vault singleton’s custody hash changes.

## CI artifacts

Every green CI uploads release binaries for:

- macOS universal (`aarch64` + `x86_64`)
- Windows `x86_64`
- Linux `x86_64` and `aarch64`

GitHub Release tags (`v*`) also attach those artifacts to the release.
