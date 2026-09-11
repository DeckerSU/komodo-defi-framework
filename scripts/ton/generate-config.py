#!/usr/bin/env python3
"""Create a private, file-based KDF configuration for the TON integration runtime."""

import json
import os
import secrets
import sys
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 6:
        print("usage: generate-config.py MODE SEED_FILE CONFIG_FILE DB_DIR RPC_PORT", file=sys.stderr)
        return 2
    mode, seed_file, config_file, db_dir, rpc_port = sys.argv[1:]
    if mode not in {"hd", "iguana"}:
        print("MODE must be hd or iguana", file=sys.stderr)
        return 2
    seed = Path(seed_file).read_text(encoding="utf-8").strip()
    if not seed:
        print("seed file is empty", file=sys.stderr)
        return 2

    # KDF owns BIP39 processing for HD mode. Iguana intentionally uses the
    # same supplied KDF passphrase input but TON consumes the resulting 32-byte
    # Iguana private-key material rather than a native TON mnemonic.
    config = {
        "gui": "nogui",
        "netid": 7777 if mode == "hd" else 7778,
        "rpc_password": secrets.token_urlsafe(32),
        "passphrase": seed,
        "dbdir": db_dir,
        "rpcport": int(rpc_port),
        "i_am_seed": False,
        "rpcip": "127.0.0.1",
    }
    destination = Path(config_file)
    destination.parent.mkdir(parents=True, exist_ok=True)
    old_mask = os.umask(0o077)
    try:
        destination.write_text(json.dumps(config), encoding="utf-8")
        destination.chmod(0o600)
    finally:
        os.umask(old_mask)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
