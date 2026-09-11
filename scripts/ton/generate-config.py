#!/usr/bin/env python3
"""Create a private, file-based KDF configuration for the TON integration runtime."""

import json
import os
import secrets
import sys
from pathlib import Path


def seed_nodes_for_netid(seed_nodes_file: Path, netid: int) -> list[str]:
    try:
        entries = json.loads(seed_nodes_file.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read seed nodes: {error}") from error

    if not isinstance(entries, list):
        raise ValueError("seed nodes must be a JSON array")

    nodes = sorted(
        {
            entry["host"]
            for entry in entries
            if isinstance(entry, dict)
            and entry.get("netid") == netid
            and isinstance(entry.get("host"), str)
            and entry["host"]
        }
    )
    if not nodes:
        raise ValueError(f"no seed nodes configured for netid {netid}")
    return nodes


def main() -> int:
    if len(sys.argv) != 7:
        print("usage: generate-config.py MODE SEED_FILE SEED_NODES_FILE CONFIG_FILE DB_DIR RPC_PORT", file=sys.stderr)
        return 2
    mode, seed_file, seed_nodes_file, config_file, db_dir, rpc_port = sys.argv[1:]
    if mode not in {"hd", "iguana"}:
        print("MODE must be hd or iguana", file=sys.stderr)
        return 2
    seed = Path(seed_file).read_text(encoding="utf-8").strip()
    if not seed:
        print("seed file is empty", file=sys.stderr)
        return 2
    try:
        seed_nodes = seed_nodes_for_netid(Path(seed_nodes_file), 6133)
    except ValueError as error:
        print(error, file=sys.stderr)
        return 2

    # KDF owns BIP39 processing for HD mode. Iguana intentionally uses the
    # same supplied KDF passphrase input but TON consumes the resulting 32-byte
    # Iguana private-key material rather than a native TON mnemonic.
    config = {
        "gui": "nogui",
        # The shared KDF test network used by this runtime. The modes use
        # separate databases and RPC ports, so they can safely share netid.
        "netid": 6133,
        # KDF validates that the RPC password has a special character. URL-safe
        # random output can consist solely of letters and digits, so append a
        # known punctuation character to make every generated config valid.
        "rpc_password": f"{secrets.token_urlsafe(32)}!",
        "passphrase": seed,
        # KDF only builds GlobalHDAccountCtx when this is enabled. TON's HD
        # implementation then derives its fixed SLIP-10 path from that normal
        # BIP39 context; it does not introduce a TON-only mnemonic mode.
        "enable_hd": mode == "hd",
        "dbdir": db_dir,
        "rpcport": int(rpc_port),
        # Run as a normal light node. The KDF P2P precheck requires configured
        # seed nodes when neither a seed nor a bootstrap node is selected.
        "disable_p2p": False,
        "i_am_seed": False,
        "is_bootstrap_node": False,
        "seednodes": seed_nodes,
        "myipaddr": "127.0.0.1",
        "rpcip": "127.0.0.1",
        # Enable the native SSE endpoint used by the balance and history
        # streaming checks. The runtime listens on loopback only.
        "event_streaming_configuration": {
            "access_control_allow_origin": "http://127.0.0.1",
        },
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
