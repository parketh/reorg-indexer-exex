# reorg-indexer-exex

A minimal reorg-aware indexer built with [reth](https://github.com/paradigmxyz/reth) Execution Extensions (ExEx).

When the chain reorgs, it removes the DB rows for the dropped blocks and indexes new blocks in an atomic transaction.

## Usage

```sh
cargo test
```

`tests/erc20.rs` shows an e2e example of indexing a ERC-20 `Transfer` event.