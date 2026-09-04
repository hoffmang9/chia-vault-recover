# Chia Vault Recover

Recover a [Chia Cloud Wallet](https://www.chia.net/) vault using the BIP39 recovery passphrase, then rekey custody to a new BLS mnemonic.

Licensed under the [Apache License 2.0](LICENSE).

## What you need

1. **The vault Receive address** — the bech32m `xch1…` or `txch1…` address shown in Cloud Wallet. Start here. The first run only looks up the launcher and checks whether a prior custody spend is on chain. You do **not** enter the recovery phrase for this check.
2. **Recovery passphrase** — the 24-word phrase Cloud Wallet gave you for vault recovery. Needed to **Start recovery** (to rebuild the public layout and to sign). You may also enter it after lookup to verify a clawback; it is never written to the lookup cache.
3. **A new custody mnemonic** — 24 words by default (12 optional); this becomes the post-recovery spend key. The tool can auto-generate a second mnemonic for the new recovery branch. Also only needed when you start.

You also need network access (coinset by default, or a full node) to find the vault singleton and broadcast transactions.

**You usually do not need a `vault-config-*.json` file.** That download is a fallback when this vault has never published its layout on chain. See [If lookup says you need a vault-config](#if-lookup-says-you-need-a-vault-config).

## Recover a vault

### GUI

```bash
chia-vault-recover-gui
```

1. Paste the vault Receive address and click **Look up vault**. This does not ask for the recovery phrase. A successful lookup is saved on disk (see [Lookup cache](#lookup-cache)). You can close the app and come back later; the next launch skips the chain search.
2. If lookup asks for a self-send or a vault-config, follow the on-screen steps (same as the CLI notes below).
3. Optionally enter the clawback window and/or recovery phrase and click **Check clawback now**. This is not required. Without the phrase, a typed clawback is saved only as a hint. With the phrase, a matching clawback is saved as verified. The phrase is never written to disk.
4. When you are ready to start, paste the recovery phrase (if you have not already) and a new custody mnemonic, then **Start recovery**. If you know the clawback window in seconds, enter it; otherwise the app uses a verified cache value, then a hint, then common Cloud Wallet values (including 43200 / 12 hours) until the spend matches the chain.
5. After the clawback window, **Finish recovery**.

`xch1…` / `txch1…` selects mainnet or testnet11 automatically.

### CLI

#### 1. Look up the address

```bash
chia-vault-recover lookup --vault xch1...
```

`txch1…` selects testnet11; `xch1…` selects mainnet. A hex launcher id still works if you have one.

The tool:

1. Decodes the Receive address
2. Finds the vault launcher id from spent coins at that address (or their parents)
3. Walks the vault singleton and looks for a previous **custody** spend
4. Tells you whether this vault can be recovered later, or whether a vault-config JSON is required

You should see: *This vault can be recovered.* The lookup is written to the [lookup cache](#lookup-cache). The recovery phrase is not needed until `start` (or an optional clawback check).

To store a clawback hint, or to verify one if you also pass the phrase:

```bash
chia-vault-recover lookup --vault xch1... --clawback-secs 43200
chia-vault-recover lookup --vault xch1... --clawback-secs 43200 \
  --recovery-mnemonic-file recovery.txt
```

The recovery phrase is used only in memory. It is never written to the cache.

#### 2. If lookup cannot find a custody spend

The Receive address does **not** contain the launcher id. An unused vault, or a vault that has only ever received funds, has nothing on chain for this tool to parse.

**If you can still open the vault at [vault.chia.net](https://vault.chia.net):**

1. **Preferred — send any amount from the vault back to the same Receive address** (or any address you control). A self-send is enough. Cloud Wallet spends the vault singleton with your passkey or the Chia Signer App, which publishes the launcher id and the custody path. Wait for that transaction to confirm, then run `lookup` on the same address again.
2. **Or download the public vault-config JSON** without sending: while logged in, open DevTools → Console (macOS: Option-Command-J; Windows/Linux: Ctrl+Shift+J), paste [`scripts/download-vault-config.js`](scripts/download-vault-config.js), and press Enter. Then:

```bash
chia-vault-recover inspect --config vault-config-….json
```

If you cannot access Cloud Wallet, you need a `vault-config-*.json` you saved earlier.

#### 3. Start delayed recovery

```bash
chia-vault-recover start \
  --vault xch1... \
  --recovery-mnemonic-file recovery.txt \
  --new-custody-mnemonic-file new-custody.txt \
  --out-config post-recovery-vault-config.json
```

`start --vault` reuses the lookup cache when present (no chain walk). Otherwise it looks up the vault, writes the cache, rebuilds the public layout from the recovery phrase, then signs. If you know the current clawback window, pass `--clawback-secs` (for example `43200`). If you omit it, a verified cache value is used; then a hint (tried first, then defaults); then common Cloud Wallet values until the reconstructed spend matches the chain. The vault enters RECOVERY (the old passkey or Chia Signer App can still claw back during the window). Run `lookup` again to refresh a stale cache.

Or pass `--config vault-config.json` if you already have a downloaded file (that file already includes the timelock).

#### 4. Wait, then finish

Wait `clawbackTimelock` seconds (often 43200 / 12h):

```bash
chia-vault-recover finish \
  --config vault-config.json \
  --post-recovery-config post-recovery-vault-config.json
```

Default destination: new **24-word** BLS custody + an **auto-generated** second BLS recovery mnemonic (12-word option available). The generated recovery mnemonic is shown once (CLI print / GUI clipboard) and is **not** written into the public post-recovery config file.

## If lookup says you need a vault-config

A **Download Config** button is coming soon to [vault.chia.net](https://vault.chia.net). Until then, the browser script exports the same public file the API already returns while you are logged in.

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

Prefer a self-send (step 2 above) when you still have Cloud Wallet access and just need this tool to see the vault. The script is the right choice when you want a saved copy of the layout, or you cannot wait for a spend to confirm.

## Install / build

```bash
cargo build --release
# binaries: target/release/chia-vault-recover
#           target/release/chia-vault-recover-gui
```

## CLI reference

```bash
# First run: address only (can this vault be recovered later?)
chia-vault-recover lookup --vault xch1...

# Optional: save a clawback hint, or verify it with the recovery phrase
chia-vault-recover lookup --vault xch1... --clawback-secs 43200
chia-vault-recover lookup --vault xch1... --clawback-secs 43200 \
  --recovery-mnemonic-file recovery.txt

# Later: start delayed recovery (phrase required here)
chia-vault-recover start \
  --vault xch1... \
  --recovery-mnemonic-file recovery.txt \
  --new-custody-mnemonic-file new-custody.txt

# Optional: pass the current clawback window if you know it
chia-vault-recover start \
  --vault xch1... \
  --clawback-secs 43200 \
  --recovery-mnemonic-file recovery.txt \
  --new-custody-mnemonic-file new-custody.txt

# After the clawback window
chia-vault-recover finish \
  --config vault-config.json \
  --post-recovery-config post-recovery-vault-config.json

# Fallback: verify a downloaded JSON
chia-vault-recover inspect --config vault-config.json
```

`lookup` aliases: `discover`, `resolve`.

Useful flags:

| Flag | Meaning |
|------|---------|
| `--vault` | Receive address (`xch1…` / `txch1…`) or launcher id. Aliases: `--address`, `--launcher-id` |
| `--network mainnet\|testnet11` | Used when the input is a hex launcher id. Addresses pick the network from `xch` / `txch` |
| `--backend coinset\|rpc` | Default **coinset**; with `rpc` set `--full-node-url` |
| `--word-count 12\|24` | Length for auto-generated recovery mnemonic (default 24) |
| `--clawback-secs` | On `lookup`, saved as a hint unless a recovery phrase is also given (then verified). On `start --vault`, an explicit value is tried alone; if omitted, a verified cache value, then a hint, then common Cloud Wallet values (including 43200 / 12h) |

Fees are not supported yet (zero-fee spends only).

Mnemonics may also be passed via env: `CHIA_VAULT_RECOVERY_MNEMONIC`, `CHIA_VAULT_NEW_CUSTODY_MNEMONIC`.

## What it does on chain

Cloud Wallet vaults are MIPS 1-of-2 singletons (custody | recovery). This tool runs **delayed (timelocked) recovery**:

1. **lookup** — address → launcher → prior custody spend (no recovery phrase); writes the lookup cache
2. **start** — reuse cache when present; recovery phrase (+ optional `--clawback-secs`, else cache / common values) → rebuild public layout and sign; vault enters RECOVERY
3. wait for `clawbackTimelock` seconds
4. **finish** — permissionless rekey to the new custody configuration

## Key derivation (Cloud Wallet compatible)

```
BIP39 mnemonic → seed("") → AugSchemeMPL.keyGen / SecretKey::from_seed
```

No `m/12381/8444/...` path. Matches Cloud Wallet `bls.ts`.

## Lookup cache

A successful lookup writes the public chain facts (launcher, custody path, current coin, ancestor puzzle hashes) to a JSON file shared by the GUI and CLI. The recovery phrase is never stored.

Default path (macOS, Windows, and Linux): `~/.chia-vault-recover/lookup-cache.json`. Override with `CHIA_VAULT_RECOVER_CACHE`.

On GUI launch, the last saved vault is loaded so you can Start recovery without searching again. `start --vault` does the same. Run **Look up vault** / `lookup` again to refresh from the chain.

A clawback value is stored only when you supply one:

- Without the recovery phrase: saved as a **hint** (tried first at Start, then the usual defaults)
- With the recovery phrase: checked against the chain and saved as **verified** when it matches

## Testing

CI runs **Simulator end-to-end** recovery tests (real puzzles and signatures via `chia-sdk-test`). No live-network CI.

```bash
cargo test --all
```

### Manual testnet11 recipe

1. Look up a testnet vault address (`txch1…`).
2. If lookup asks for a self-send, send a dust amount back to the same address from Cloud Wallet and wait for confirmation.
3. Ensure the vault singleton is unspent (zero-fee spends only).
4. Run:

```bash
chia-vault-recover lookup --vault txch1...
chia-vault-recover start --vault txch1... --network testnet11 \
  --recovery-mnemonic-file recovery.txt \
  --new-custody-mnemonic-file new-custody.txt
# wait clawbackTimelock
chia-vault-recover finish --config vault-config.json --network testnet11 \
  --post-recovery-config post-recovery-vault-config.json
```

Or point `--backend rpc --full-node-url https://localhost:8555` at a synced full node.

## Caveats

- Instant recovery (spend/passkey or Chia Signer App key) is out of scope — use delayed recovery with the passphrase.
- A never-spent Receive address cannot yield a launcher id. One Cloud Wallet send (including a self-send) is enough.
- `lookup` also needs a previous **custody** spend of the current vault configuration. An unspent eve singleton, or a vault that has only ever been spent via recovery, cannot reveal the custody path. Cloud Wallet has not shipped on-chain config hints (`TEMP_VAULT_CONFIG_EXPORT` is still the JSON export/restore path).
- Clawback during the window still requires the old custody passkey or Chia Signer App (not implemented here).
- After finish, custody is on-chain BLS; Cloud Wallet’s product UI may not re-import that vault as a normal passkey or Chia Signer App vault.
- p2-singleton XCH/CATs are unchanged; only the vault singleton’s custody hash changes.

## CI artifacts

Every green CI uploads release binaries for:

- macOS universal (`aarch64` + `x86_64`)
- Windows `x86_64`
- Linux `x86_64` and `aarch64`

GitHub Release tags (`v*`) also attach those artifacts to the release.
