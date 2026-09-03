/**
 * Fallback: download Cloud Wallet vault-config JSON from a logged-in tab.
 *
 * Prefer chia-vault-recover lookup --vault xch1… first. You only need this
 * file if lookup says the vault has not yet published its layout on chain
 * and you do not want to (or cannot) send a small self-transfer from
 * vault.chia.net.
 *
 * Cloud Wallet does not yet expose a download button. Paste this file
 * into the browser console while logged in.
 *
 * Usage:
 *   1. Log in at https://vault.chia.net (or the testnet Cloud Wallet host).
 *   2. Open the vault you want, or stay on the vaults list to export all.
 *   3. Open DevTools → Console (macOS: ⌥⌘J, Windows/Linux: Ctrl+Shift+J).
 *   4. Paste this entire file and press Enter.
 *   5. One download: vault-config-*.json for a single vault, or
 *      vault-configs.json ({ vaults: [...] }) for the whole list.
 *      Public keys and layout only — not your recovery phrase.
 *
 * Then verify before recovery (single-vault file):
 *   chia-vault-recover inspect --config vault-config-….json
 */
(async () => {
  const hex = (value, label) => {
    if (value == null) {
      throw new Error(`missing ${label}`);
    }
    return `0x${String(value).replace(/^0x/i, '')}`;
  };

  const readJson = async (response, label) => {
    const text = await response.text();
    let body;
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      throw new Error(
        `Expected JSON from ${label} (HTTP ${response.status}). Log in at vault.chia.net and run this again.`,
      );
    }
    if (!response.ok) {
      if (response.status === 401 || response.status === 403) {
        throw new Error('Session expired. Log in at vault.chia.net and run this again.');
      }
      throw new Error(`${label} failed (HTTP ${response.status})`);
    }
    return body;
  };

  const settingsResponse = await fetch('/settings.json', { credentials: 'include' });
  const settings = await readJson(settingsResponse, '/settings.json');
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
    const body = await readJson(response, graphqlUri);
    if (body.errors?.length) {
      throw new Error(body.errors.map((error) => error.message).join('; '));
    }
    if (body.data == null) {
      throw new Error('GraphQL returned no data. Log in at vault.chia.net and run this again.');
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

  const connectionEdges = (connection, field, walletId) => {
    if (connection == null) {
      return [];
    }
    if (!Array.isArray(connection.edges)) {
      throw new Error(`Incomplete vault configuration for ${walletId}: missing ${field}`);
    }
    return connection.edges;
  };

  const membersFrom = (keys, authorized, walletId) => {
    const keyMembers = connectionEdges(keys, 'key connection', walletId).map(({ node }) => {
      if (node?.publicKey == null || node?.curve == null) {
        throw new Error(
          `Incomplete vault configuration for ${walletId}: key missing publicKey/curve`,
        );
      }
      return {
        type: 'publicKey',
        publicKey: hex(node.publicKey, `publicKey for ${walletId}`),
        curve: node.curve,
      };
    });
    const vaultMembers = connectionEdges(authorized, 'authorized-wallet connection', walletId)
      .map(({ node }) => node?.custodyConfig?.vaultCustodyConfig?.vaultLauncherId)
      .filter((id) => id != null)
      .map((launcherId) => ({
        type: 'vault',
        launcherId: hex(launcherId, `authorized launcher for ${walletId}`),
      }));
    return [...keyMembers, ...vaultMembers];
  };

  const configFromWallet = (wallet) => {
    if (wallet?.id == null) {
      return null;
    }
    const vault = wallet.custodyConfig?.vaultCustodyConfig;
    if (vault?.vaultLauncherId == null) {
      return null;
    }
    if (vault.custodyThreshold == null) {
      throw new Error(`Incomplete vault configuration for ${wallet.id}: missing custodyThreshold`);
    }
    if (vault.recoveryThreshold == null) {
      throw new Error(`Incomplete vault configuration for ${wallet.id}: missing recoveryThreshold`);
    }
    if (vault.recoveryClawbackTimelock == null) {
      throw new Error(
        `Incomplete vault configuration for ${wallet.id}: missing recoveryClawbackTimelock`,
      );
    }
    const safeName = String(wallet.name || wallet.id).replaceAll(/[^a-zA-Z0-9-_]/g, '_');
    return {
      filename: `vault-config-${safeName}.json`,
      json: {
        launcherId: hex(vault.vaultLauncherId, `launcher id for ${wallet.id}`),
        custody: {
          threshold: vault.custodyThreshold,
          members: membersFrom(vault.custodyKeys, vault.custodyAuthorizedWallets, wallet.id),
        },
        recovery: {
          threshold: vault.recoveryThreshold,
          clawbackTimelock: vault.recoveryClawbackTimelock,
          members: membersFrom(vault.recoveryKeys, vault.recoveryAuthorizedWallets, wallet.id),
        },
      },
    };
  };

  const downloadJson = (filename, value) => {
    const blob = new Blob([JSON.stringify(value, null, 2)], { type: 'application/json' });
    const link = document.createElement('a');
    link.href = URL.createObjectURL(blob);
    link.download = filename;
    link.click();
    URL.revokeObjectURL(link.href);
  };

  const fromUrl = location.pathname.match(/\/wallet\/(Wallet_[^/]+)/)?.[1];
  const wallets = [];
  let after = null;
  for (;;) {
    const variables = { first: 50 };
    if (after) {
      variables.after = after;
    }
    const data = await gql(
      `query VaultConfigAssemble($first: Int!, $after: String) {
        viewer {
          wallets(first: $first, after: $after) {
            pageInfo { hasNextPage endCursor }
            edges { node { ${vaultFields} } }
          }
        }
      }`,
      variables,
    );
    const connection = data.viewer?.wallets;
    if (connection == null) {
      throw new Error('Not signed in. Log in at vault.chia.net and run this again.');
    }
    if (!Array.isArray(connection.edges)) {
      throw new Error('Incomplete wallet list from GraphQL (missing wallets.edges)');
    }
    for (const edge of connection.edges) {
      if (edge?.node) {
        wallets.push(edge.node);
      }
    }
    if (fromUrl && wallets.some((wallet) => wallet.id === fromUrl)) {
      break;
    }
    if (!connection.pageInfo?.hasNextPage || !connection.pageInfo.endCursor) {
      break;
    }
    after = connection.pageInfo.endCursor;
  }

  const selected = fromUrl ? wallets.filter((wallet) => wallet.id === fromUrl) : wallets;
  if (fromUrl && selected.length === 0) {
    throw new Error(
      `Wallet ${fromUrl} was not in this account. Open that vault while logged in and run this again.`,
    );
  }

  const configs = [];
  for (const wallet of selected) {
    const config = configFromWallet(wallet);
    if (config) {
      configs.push(config);
    } else if (fromUrl) {
      throw new Error(`Incomplete vault configuration for ${fromUrl}: missing launcher id`);
    }
  }

  if (!configs.length) {
    throw new Error(
      'No vaults found. Log in at vault.chia.net, open a vault or the vaults list, and run this again.',
    );
  }

  if (configs.length === 1) {
    downloadJson(configs[0].filename, configs[0].json);
  } else {
    downloadJson(
      'vault-configs.json',
      { vaults: configs.map((config) => config.json) },
    );
    console.warn(
      'Downloaded vault-configs.json (one file). inspect --config needs a single vault object: copy one entry from vaults[] into its own file.',
    );
  }

  console.table(
    configs.map((config) => ({ file: config.filename, launcherId: config.json.launcherId })),
  );
  return configs;
})();
