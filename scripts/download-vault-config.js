/**
 * Download Cloud Wallet vault-config JSON from a logged-in browser tab.
 *
 * Cloud Wallet does not yet expose a download button on vault.chia.net.
 * GUI support for this export is coming soon. Until then, paste this file
 * into the browser console while logged in.
 *
 * Usage:
 *   1. Log in at https://vault.chia.net (or the testnet Cloud Wallet host).
 *   2. Open the vault you want, or stay on the vaults list to export all.
 *   3. Open DevTools → Console (macOS: ⌥⌘J, Windows/Linux: Ctrl+Shift+J).
 *   4. Paste this entire file and press Enter.
 *   5. The browser downloads vault-config-*.json. Keep those files with
 *      your backups. They contain public keys and layout only — not your
 *      recovery phrase or any private key.
 *
 * Then verify before recovery:
 *   chia-vault-recover inspect --config vault-config-….json
 */
(async () => {
  const hex = (value) => {
    if (value == null) {
      throw new Error('missing hex field');
    }
    return `0x${String(value).replace(/^0x/i, '')}`;
  };

  const settings = await (await fetch('/settings.json')).json();
  const graphqlUri = settings.GRAPHQL_URI;
  if (!graphqlUri) {
    throw new Error('Could not find GRAPHQL_URI in /settings.json. Are you on vault.chia.net?');
  }

  const gql = async (query, variables) => {
    const response = await fetch(graphqlUri, {
      method: 'POST',
      credentials: 'include',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ query, variables }),
    });
    const body = await response.json();
    if (body.errors?.length) {
      throw new Error(body.errors.map((error) => error.message).join('; '));
    }
    return body.data;
  };

  const vaultFields = `
    id
    name
    custodyConfig {
      vaultCustodyConfig {
        vaultLauncherId
        custodyThreshold
        recoveryThreshold
        recoveryClawbackTimelock
        custodyKeys { edges { node { publicKey curve } } }
        recoveryKeys { edges { node { publicKey curve } } }
        custodyAuthorizedWallets {
          edges { node { custodyConfig { vaultCustodyConfig { vaultLauncherId } } } }
        }
        recoveryAuthorizedWallets {
          edges { node { custodyConfig { vaultCustodyConfig { vaultLauncherId } } } }
        }
      }
    }
  `;

  const fromUrl = location.pathname.match(/\/wallet\/(Wallet_[^/]+)/)?.[1];
  let wallets;
  if (fromUrl) {
    const data = await gql(
      `query VaultConfigAssembleOne($id: ID!) { wallet(id: $id) { ${vaultFields} } }`,
      { id: fromUrl },
    );
    wallets = [data.wallet];
  } else {
    const data = await gql(
      `query VaultConfigAssembleAll {
        viewer { wallets(first: 50) { edges { node { ${vaultFields} } } } }
      }`,
    );
    wallets = data.viewer.wallets.edges.map((edge) => edge.node);
  }

  const toMembers = (keys, authorized) => [
    ...keys.edges.map(({ node }) => ({
      type: 'publicKey',
      publicKey: hex(node.publicKey),
      curve: node.curve,
    })),
    ...authorized.edges
      .map(({ node }) => node.custodyConfig?.vaultCustodyConfig?.vaultLauncherId)
      .filter(Boolean)
      .map((launcherId) => ({ type: 'vault', launcherId: hex(launcherId) })),
  ];

  const configs = [];
  for (const wallet of wallets) {
    const vault = wallet?.custodyConfig?.vaultCustodyConfig;
    if (!vault?.vaultLauncherId) {
      continue;
    }
    if (
      vault.custodyThreshold == null ||
      vault.recoveryThreshold == null ||
      vault.recoveryClawbackTimelock == null
    ) {
      throw new Error(`Incomplete vault configuration for ${wallet.id}`);
    }
    configs.push({
      filename: `vault-config-${(wallet.name || wallet.id).replaceAll(/[^a-zA-Z0-9-_]/g, '_')}.json`,
      json: {
        launcherId: hex(vault.vaultLauncherId),
        custody: {
          threshold: vault.custodyThreshold,
          members: toMembers(vault.custodyKeys, vault.custodyAuthorizedWallets),
        },
        recovery: {
          threshold: vault.recoveryThreshold,
          clawbackTimelock: vault.recoveryClawbackTimelock,
          members: toMembers(vault.recoveryKeys, vault.recoveryAuthorizedWallets),
        },
      },
    });
  }

  if (!configs.length) {
    throw new Error(
      'No vaults found. Log in at vault.chia.net, open a vault or the vaults list, and run this again.',
    );
  }

  for (const { filename, json } of configs) {
    const blob = new Blob([JSON.stringify(json, null, 2)], { type: 'application/json' });
    const link = document.createElement('a');
    link.href = URL.createObjectURL(blob);
    link.download = filename;
    link.click();
    URL.revokeObjectURL(link.href);
  }

  console.table(configs.map((config) => ({ file: config.filename, launcherId: config.json.launcherId })));
  return configs;
})();
