/**
 * Download Cloud Wallet vault-config JSON from a logged-in vault page.
 *
 * Cloud Wallet does not yet expose a download button on vault.chia.net.
 * Paste this file into the browser console while logged in on a vault.
 *
 * Usage:
 *   1. Log in at https://vault.chia.net (or the testnet Cloud Wallet host).
 *   2. Open the vault you want to export.
 *   3. Open DevTools → Console (macOS: ⌥⌘J, Windows/Linux: Ctrl+Shift+J).
 *   4. Paste this entire file and press Enter.
 *   5. The browser downloads one vault-config-*.json (public layout only —
 *      not your recovery phrase).
 *
 * Then verify before recovery:
 *   chia-vault-recover inspect --config vault-config-….json
 */
(async () => {
  const walletId = location.pathname.match(/\/wallet\/(Wallet_[^/]+)/)?.[1];
  if (!walletId) {
    throw new Error('Open a vault at vault.chia.net, then run this again.');
  }

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

  const connectionEdges = (connection, field, id) => {
    if (connection == null) {
      return [];
    }
    if (!Array.isArray(connection.edges)) {
      throw new Error(`Incomplete vault configuration for ${id}: missing ${field}`);
    }
    return connection.edges;
  };

  const membersFrom = (keys, authorized, id) => {
    const keyMembers = connectionEdges(keys, 'key connection', id).map(({ node }) => {
      if (node?.publicKey == null || node?.curve == null) {
        throw new Error(`Incomplete vault configuration for ${id}: key missing publicKey/curve`);
      }
      return {
        type: 'publicKey',
        publicKey: hex(node.publicKey, `publicKey for ${id}`),
        curve: node.curve,
      };
    });
    const vaultMembers = connectionEdges(authorized, 'authorized-wallet connection', id)
      .map(({ node }) => node?.custodyConfig?.vaultCustodyConfig?.vaultLauncherId)
      .filter((launcherId) => launcherId != null)
      .map((launcherId) => ({
        type: 'vault',
        launcherId: hex(launcherId, `authorized launcher for ${id}`),
      }));
    return [...keyMembers, ...vaultMembers];
  };

  const configFromWallet = (wallet) => {
    if (wallet?.id == null) {
      throw new Error(`Wallet ${walletId} was not in this account. Open that vault while logged in.`);
    }
    const vault = wallet.custodyConfig?.vaultCustodyConfig;
    if (vault?.vaultLauncherId == null) {
      throw new Error(`Incomplete vault configuration for ${wallet.id}: missing launcher id`);
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

  const data = await gql(
    `query VaultConfigAssemble($id: ID!) {
      wallet(id: $id) {
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
      }
    }`,
    { id: walletId },
  );

  const { filename, json } = configFromWallet(data.wallet);
  const blob = new Blob([JSON.stringify(json, null, 2)], { type: 'application/json' });
  const link = document.createElement('a');
  link.href = URL.createObjectURL(blob);
  link.download = filename;
  link.click();
  URL.revokeObjectURL(link.href);
  console.table([{ file: filename, launcherId: json.launcherId }]);
  return json;
})();
