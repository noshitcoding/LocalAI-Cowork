"""Operator-only catalog publication; never exposed as a workload command."""
import argparse
import json
from pathlib import Path

import provisioner as p
from release_catalog import ReleaseCatalog, validate_release


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--manifest", help="operator-reviewed immutable release manifest")
    action.add_argument("--withdraw", help="release hash to remove from future selections")
    args = parser.parse_args()
    config = json.loads(Path(args.config).read_text(encoding="utf-8"))
    with p.operator_lock(Path(config["state_directory"])):
        catalog = ReleaseCatalog(config, p.private_write)
        if args.withdraw:
            catalog.withdraw(args.withdraw)
            print(json.dumps({"withdrawn": args.withdraw}))
        else:
            value = json.loads(Path(args.manifest).read_text(encoding="utf-8"))
            validate_release(value, value.get("id") if isinstance(value, dict) else None)
            catalog.publish(value["name"], value["images"], value["supported_sources"])
            print(json.dumps({"published": value["id"]}))


if __name__ == "__main__":
    main()
