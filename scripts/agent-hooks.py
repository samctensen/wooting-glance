"""Install or remove only Glance's lifecycle hooks for local coding sessions."""
import argparse
import json
import os
from pathlib import Path
import shlex
import tempfile


def configure(path, service, binary, remove):
    if remove and not path.exists():
        return
    original = path.read_bytes() if path.exists() else None
    config = json.loads(original) if original else {}
    if service == "muse":
        config.setdefault("schema_version", 1)
    hooks = config.setdefault("hooks", {})
    events = ["UserPromptSubmit", "PermissionRequest", "PostToolUse", "Stop", "SessionEnd"]
    events += ["SubagentStart", "SubagentStop"]
    events += ["StopFailure", "PostToolUseFailure"] if service in ("claude", "muse") else ["Interrupt"]
    if service == "grok":
        events = ["UserPromptSubmit", "PostToolUse", "PostToolUseFailure", "PermissionDenied",
                  "Notification", "SubagentStart", "SubagentStop", "Stop", "StopFailure",
                  "StopCancelled", "SessionEnd"]
    group = {"hooks": [{"type": "command", "command": shlex.join([str(binary), "hook", service]), "timeout": 2}]}
    changed = False
    for event in events:
        groups = hooks.get(event, [])
        if remove and group in groups:
            groups.remove(group)
            if not groups:
                del hooks[event]
            changed = True
        elif not remove and group not in groups:
            hooks.setdefault(event, []).append(group)
            changed = True
    if changed:
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as tmp:
            tmp.write((json.dumps(config, indent=2, ensure_ascii=False) + "\n").encode())
        try:
            if (path.read_bytes() if path.exists() else None) != original:
                raise SystemExit(f"{path} changed during setup. Retry.")
            os.replace(tmp.name, path)
        finally:
            Path(tmp.name).unlink(missing_ok=True)
    print(f"{service}: {path}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--remove", action="store_true")
    parser.add_argument("--service", choices=["claude", "codex", "grok", "muse"],
                        help="configure one service (default: Claude and Codex)")
    args = parser.parse_args()
    binary = Path(__file__).resolve().parents[1] / "target/release/glance"
    if not args.remove and not binary.is_file():
        parser.error("Build the helper first with cargo build --release --locked")
    for service, env, default, filename in [
        ("claude", "CLAUDE_CONFIG_DIR", ".claude", "settings.json"),
        ("codex", "CODEX_HOME", ".codex", "hooks.json"),
        ("muse", "XDG_CONFIG_HOME", ".config", "muse/settings.json"),
        ("grok", "GROK_HOME", ".grok", "hooks/wooting-glance.json"),
    ]:
        if service not in ([args.service] if args.service else ["claude", "codex"]):
            continue
        config_dir = Path(os.environ.get(env) or Path.home() / default)
        configure(config_dir / filename, service, binary, args.remove)
