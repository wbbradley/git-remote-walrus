# Development

## Getting a Sui + Walrus localnet running

After installing `sui` with `suiup install sui`, you can start a local sui network with:

```bash
# Start a local sui network.
RUST_LOG="on,error" sui start --with-faucet --force-regenesis
```

To bring up a local Walrus network, run:

```bash
# Get the Walrus sources (NB: running local walrus clusters for now requires some scripts only
# available via the git repo).
git clone https://github.com/MystenLabs/walrus.git
cd walrus

# You can re-run the below steps after (re)starting your local sui network as needed.
rm -rf working_dir/ contracts/*/build
./scripts/local-testbed.sh -fn localnet
```

The above `local-testbed.sh` script will create a `working_dir/` directory with the sui client
configuration and the Walrus contracts built and deployed to your local sui network.

```bash
# In another terminal, you can now access the local sui network faucet with the Walrus local-testnet
# accounts.
cd walrus
sui client --client.config working_dir/sui_client.yaml faucet --url http://127.0.0.1:9123/gas
```

## git-remote-walrus cheatsheet

Now that you have a local sui + walrus network running, you can use the `git-remote-walrus` tool to
deploy its remote_state package to your local sui network.

```bash
cd git-remote-walrus  # This repo's directory.
git-remote-walrus deploy
```

The above command will build the `remote_state` package and deploy it to your local sui network, and
then give you instructions on how to proceed.
