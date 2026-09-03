# Chia Vault Recover

Recover a [Chia Cloud Wallet](https://www.chia.net/) vault using the BIP39 recovery passphrase, then rekey custody to a new BLS mnemonic.

Licensed under the [Apache License 2.0](LICENSE).

## What you need

1. **The vault Receive address** — the bech32m `xch1…` or `txch1…` address shown in Cloud Wallet. Start here. The tool looks up the launcher and checks whether the public vault layout is already on chain.
2. **Recovery passphrase** — the 24-word phrase Cloud Wallet gave you for vault recovery (signs delayed recovery).
3. **A new custody mnemonic** — 24 words by default (12 optional); this becomes the post-recovery spend key. The tool can auto-generate a second mnemonic for the new recovery branch.

You also need network access (coinset by default, or a full node) to find the vault singleton and broadcast transactions.

**You usually do not need a `vault-config-*.json` file.** That download is a fallback when this vault has never published its layout on chain. See [If lookup says you need a vault-config](#if-lookup-says-you-need-a-vault-config).

## Recover a vault

### 1. Look up the address

```bash
chia-vault-recover lookup --vault xch1...
```

`txch1…` selects testnet11; `xch1…` selects mainnet. A hex launcher id still works if you have one.

The tool:

1. Decodes the Receive address
2. Finds the vault launcher id from spent coins at that address (or their parents)
3. Walks the vault singleton and looks for a previous **custody** spend
4. Tells you whether a vault-config JSON is required

If the layout is already on chain, look up again with the recovery phrase to write a local public config (keys and timelock only — not the phrase):

```bash
chia-vault-recover lookup \
  --vault xch1... \
  --recovery-mnemonic-file recovery.txt \
  --out-config vault-config.json
```

You should see: *Vault layout rebuilt from chain. You do not need a vault-config-*.json download.*

### 2. If lookup cannot rebuild the layout

The Receive address does **not** contain the launcher id. An unused vault, or a vault that has only ever received funds, has nothing on chain for this tool to parse.

**If you can still open the vault at [vault.chia.net](https://vault.chia.net):**

1. **Preferred — send any amount from the vault back to the same Receive address** (or any address you control). A self-send is enough. Cloud Wallet spends the vault singleton with your passkey, which publishes the launcher id and the custody path. Wait for that transaction to confirm, then run `lookup` on the same address again.
2. **Or download the public vault-config JSON** without sending: while logged in, open DevTools → Console (macOS: Option-Command-J; Windows/Linux: Ctrl+Shift+J), paste [`scripts/download-vault-config.js`](scripts/download-vault-config.js), and press Enter. Then:

```bash
chia-vault-recover inspect --config vault-config-….json
```

If you cannot access Cloud Wallet, you need a `vault-config-*.json` you saved earlier.

### 3. Start delayed recovery

```bash
chia-vault-recover start \
  --vault xch1... \
  --recovery-mnemonic-file recovery.txt \
  --new-custody-mnemonic-file new-custody.txt \
  --out-config post-recovery-vault-config.json
```

`start --vault` looks up the vault first, then signs with the recovery phrase. The vault enters RECOVERY (the old passkey can still claw back during the window).

Or pass `--config vault-config.json` if you already have a rebuilt or downloaded file.

### 4. Wait, then finish

Wait `clawbackTimelock` seconds (often 43200 / 12h):

```bash
chia-vault-recover finish \
  --config vault-config.json \
  --post-recovery-config post-recovery-vault-config.json
```

Default destination: new **24-word** BLS custody + an **auto-generated** second BLS recovery mnemonic (12-word option available). The generated recovery mnemonic is shown once (CLI print / GUI clipboard) and is **not** written into the public post-recovery config file.

## GUI

```bash
chia-vault-recover-gui
```

1. Paste the vault Receive address and click **Look up vault**.
2. If the layout is on chain, paste the recovery phrase and look up again (or paste the phrase first and look up once).
3. If lookup asks for a self-send or a vault-config, follow the on-screen steps (same as above).
4. Enter a new custody mnemonic, then **Start recovery**. After the clawback window, **Finish recovery**.

`xch1…` / `txch1…` selects mainnet or testnet11 automatically.

## If lookup says you need a vault-config

A **Download Config** button is coming soon to [vault.chia.net](https://vault.chia.net). Until then, the browser script exports the same public file the API already returns while you are logged in.

The file is public vault layout only. It does **not** include your recovery passphrase or any private key.

1. Log in at [vault.chia.net](https://vault.chia.net) (or the testnet Cloud Wallet host).
2. Open the vault you want. To export every vault, stay on the vaults list.
3. Open DevTools → Console (macOS: Option-Command-J; Windows/Linux: Ctrl+Shift+J).
4. Paste the contents of [`scripts/download-vault-config.js`](scripts/download-vault-config.js) and press Enter.
5. The browser downloads `vault-config-*.json`. Store it with your backups.
6. Confirm it matches the chain:

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
# Start here: address only (do I need a vault-config?)
chia-vault-recover lookup --vault xch1...

# Rebuild public layout from chain (no JSON download)
chia-vault-recover lookup \
  --vault xch1... \
  --recovery-mnemonic-file recovery.txt \
  --out-config vault-config.json

# Look up + start in one step
chia-vault-recover start \
  --vault xch1... \
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
| `--clawback-secs` | `lookup` / `start --vault`; if omitted, common values (including 43200) are tried |

Fees are not supported yet (zero-fee spends only).

Mnemonics may also be passed via env: `CHIA_VAULT_RECOVERY_MNEMONIC`, `CHIA_VAULT_NEW_CUSTODY_MNEMONIC`.

## What it does on chain

Cloud Wallet vaults are MIPS 1-of-2 singletons (custody | recovery). This tool runs **delayed (timelocked) recovery**:

1. **lookup** — address → launcher → optional rebuilt public layout
2. **start** — sign with the recovery phrase; vault enters RECOVERY
3. wait for `clawbackTimelock` seconds
4. **finish** — permissionless rekey to the new custody configuration

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

1. Look up a testnet vault address (`txch1…`).
2. If lookup asks for a self-send, send a dust amount back to the same address from Cloud Wallet and wait for confirmation.
3. Ensure the vault singleton is unspent (zero-fee spends only).
4. Run:

```bash
chia-vault-recover lookup --vault txch1... --recovery-mnemonic-file recovery.txt
chia-vault-recover start --vault txch1... --network testnet11 \
  --recovery-mnemonic-file recovery.txt \
  --new-custody-mnemonic-file new-custody.txt
# wait clawbackTimelock
chia-vault-recover finish --config vault-config.json --network testnet11 \
  --post-recovery-config post-recovery-vault-config.json
```

Or point `--backend rpc --full-node-url https://localhost:8555` at a synced full node.

## Caveats

- Instant recovery (spend/passkey key) is out of scope — use delayed recovery with the passphrase.
- A never-spent Receive address cannot yield a launcher id. One Cloud Wallet send (including a self-send) is enough.
- `lookup` also needs a previous **custody** spend of the current vault configuration. An unspent eve singleton, or a vault that has only ever been spent via recovery, cannot reveal the custody path. Cloud Wallet has not shipped on-chain config hints (`TEMP_VAULT_CONFIG_EXPORT` is still the JSON export/restore path).
- Clawback during the window still requires the old custody passkey (not implemented here).
- After finish, custody is on-chain BLS; Cloud Wallet’s product UI may not re-import that vault as a normal passkey vault.
- p2-singleton XCH/CATs are unchanged; only the vault singleton’s custody hash changes.

## CI artifacts

Every green CI uploads release binaries for:

- macOS universal (`aarch64` + `x86_64`)
- Windows `x86_64`
- Linux `x86_64` and `aarch64`

GitHub Release tags (`v*`) also attach those artifacts to the release.
