#!/usr/bin/env python3
"""
OpenClaw <-> Tutti action wrapper.

This script provides a stable JSON envelope around Tutti commands so an external
agent can call one binary/script entrypoint and always get machine-readable
results.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass
class CommandResult:
    ok: bool
    action: str
    command: list[str] | None
    exit_code: int
    data: Any
    stdout: str
    stderr: str
    parse_error: str | None = None
    note: str | None = None

    def to_dict(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "ok": self.ok,
            "action": self.action,
            "command": self.command,
            "exit_code": self.exit_code,
            "data": self.data,
            "stdout": self.stdout,
            "stderr": self.stderr,
        }
        if self.parse_error:
            out["parse_error"] = self.parse_error
        if self.note:
            out["note"] = self.note
        return out


def _run_tt(action: str, tt_bin: list[str], args: list[str], expect_json: bool) -> CommandResult:
    cmd = [*tt_bin, *args]
    try:
        proc = subprocess.run(cmd, text=True, capture_output=True, check=False)
    except FileNotFoundError:
        return CommandResult(
            ok=False,
            action=action,
            command=cmd,
            exit_code=127,
            data=None,
            stdout="",
            stderr="tt command not found on PATH",
        )

    data: Any = None
    parse_error: str | None = None
    if expect_json:
        payload = proc.stdout.strip()
        if payload:
            try:
                data = json.loads(payload)
            except json.JSONDecodeError as exc:
                parse_error = f"failed to parse command JSON output: {exc}"

    ok = proc.returncode == 0 and (parse_error is None or not expect_json)
    return CommandResult(
        ok=ok,
        action=action,
        command=cmd,
        exit_code=proc.returncode,
        data=data,
        stdout=proc.stdout,
        stderr=proc.stderr,
        parse_error=parse_error,
    )


def _team_status(action: str) -> CommandResult:
    state_dir = Path(".tutti") / "state"
    if not state_dir.exists():
        return CommandResult(
            ok=True,
            action=action,
            command=None,
            exit_code=0,
            data={"agents": [], "count": 0, "state_dir": str(state_dir), "exists": False},
            stdout="",
            stderr="",
            note="state directory not found",
        )

    agents: list[dict[str, Any]] = []
    parse_errors: list[str] = []
    for path in sorted(state_dir.glob("*.json")):
        # ignore non-agent state files
        if path.name in {"verify-last.json"}:
            continue
        try:
            payload = json.loads(path.read_text())
        except Exception as exc:  # pylint: disable=broad-except
            parse_errors.append(f"{path.name}: {exc}")
            continue
        if not isinstance(payload, dict):
            continue
        required = {"name", "runtime", "session_name", "status", "started_at"}
        if required.issubset(payload.keys()):
            agents.append(payload)

    return CommandResult(
        ok=True,
        action=action,
        command=None,
        exit_code=0,
        data={
            "agents": agents,
            "count": len(agents),
            "state_dir": str(state_dir),
            "exists": True,
            "parse_errors": parse_errors,
        },
        stdout="",
        stderr="",
    )


def _agent_output(action: str, tt_bin: list[str], agent: str, lines: int) -> CommandResult:
    result = _run_tt(action, tt_bin, ["peek", agent, "--lines", str(lines)], expect_json=False)
    result.data = {
        "agent": agent,
        "lines": lines,
        "output_lines": result.stdout.splitlines(),
    }
    return result


def _print_and_exit(result: CommandResult) -> None:
    print(json.dumps(result.to_dict(), indent=2))
    raise SystemExit(result.exit_code if result.exit_code >= 0 else 1)


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="tutti_openclaw.py",
        description="OpenClaw wrapper for Tutti actions",
    )
    parser.add_argument(
        "--tt-bin",
        default=os.environ.get("TUTTI_BIN", "tt"),
        help="Tutti CLI command/prefix to execute (default: tt, or TUTTI_BIN env var)",
    )
    sub = parser.add_subparsers(dest="action", required=True)

    sub.add_parser("doctor_check", help="Run tt doctor --json")
    p_perm = sub.add_parser("permissions_check", help="Run tt permissions check ... --json")
    p_perm.add_argument("command", nargs=argparse.REMAINDER, help="Command to evaluate")

    sub.add_parser("launch_team", help="Run tt up")
    p_launch = sub.add_parser("launch_agent", help="Run tt up <agent>")
    p_launch.add_argument("agent")

    sub.add_parser("list_workflows", help="Run tt run --list --json")
    p_plan = sub.add_parser("plan_workflow", help="Run tt run <workflow> --dry-run --json")
    p_plan.add_argument("workflow")
    p_plan.add_argument("--agent")
    p_plan.add_argument("--strict", action="store_true")

    p_run = sub.add_parser("run_workflow", help="Run tt run <workflow> --json")
    p_run.add_argument("workflow")
    p_run.add_argument("--agent")
    p_run.add_argument("--strict", action="store_true")

    p_verify = sub.add_parser("verify_team", help="Run tt verify --json")
    p_verify.add_argument("--workflow")
    p_verify.add_argument("--agent")
    p_verify.add_argument("--strict", action="store_true")

    sub.add_parser("read_verify_status", help="Run tt verify --last --json")
    p_gen_handoff = sub.add_parser("generate_handoff", help="Run tt handoff generate <agent> --json")
    p_gen_handoff.add_argument("agent")
    p_gen_handoff.add_argument("--reason")
    p_gen_handoff.add_argument("--ctx", type=int)
    p_apply_handoff = sub.add_parser("apply_handoff", help="Run tt handoff apply <agent>")
    p_apply_handoff.add_argument("agent")
    p_apply_handoff.add_argument("--packet")
    p_list_handoffs = sub.add_parser("list_handoffs", help="Run tt handoff list --json")
    p_list_handoffs.add_argument("--agent")
    p_list_handoffs.add_argument("--limit", type=int, default=20)
    sub.add_parser("team_status", help="Read .tutti/state/*.json")

    p_output = sub.add_parser("agent_output", help="Run tt peek <agent> --lines N")
    p_output.add_argument("agent")
    p_output.add_argument("--lines", type=int, default=100)

    p_send = sub.add_parser("send_prompt", help="Send prompt to agent with auto-up/wait/output")
    p_send.add_argument("agent")
    p_send.add_argument("prompt", nargs=argparse.REMAINDER, help="Prompt text")
    p_send.add_argument("--auto-up", action="store_true", help="Start agent if not running")
    p_send.add_argument("--wait", action="store_true", help="Wait for agent to finish")
    p_send.add_argument("--output", action="store_true", help="Capture agent response")
    p_send.add_argument("--timeout-secs", type=int, default=900)
    p_send.add_argument("--idle-stable-secs", type=int, default=5)
    p_send.add_argument("--output-lines", type=int, default=200)

    p_land = sub.add_parser("land_agent", help="Run tt land <agent>")
    p_land.add_argument("agent")
    p_land.add_argument("--pr", action="store_true", help="Push branch and open PR")
    p_land.add_argument("--force", action="store_true", help="Force land")

    p_stop = sub.add_parser("stop_agent", help="Run tt down <agent>")
    p_stop.add_argument("agent")
    sub.add_parser("stop_team", help="Run tt down")
    return parser


def main() -> None:
    parser = _build_parser()
    args = parser.parse_args()
    action = args.action
    tt_bin = shlex.split(args.tt_bin)
    if not tt_bin:
        result = CommandResult(
            ok=False,
            action=action,
            command=None,
            exit_code=2,
            data=None,
            stdout="",
            stderr="--tt-bin resolved to empty command",
        )
        _print_and_exit(result)

    if action == "doctor_check":
        _print_and_exit(_run_tt(action, tt_bin, ["doctor", "--json"], expect_json=True))

    if action == "permissions_check":
        if not args.command:
            result = CommandResult(
                ok=False,
                action=action,
                command=None,
                exit_code=2,
                data=None,
                stdout="",
                stderr="permissions_check requires a command payload",
            )
            _print_and_exit(result)
        _print_and_exit(
            _run_tt(
                action,
                tt_bin,
                ["permissions", "check", *args.command, "--json"],
                expect_json=True,
            )
        )

    if action == "launch_team":
        _print_and_exit(_run_tt(action, tt_bin, ["up"], expect_json=False))
    if action == "launch_agent":
        _print_and_exit(_run_tt(action, tt_bin, ["up", args.agent], expect_json=False))
    if action == "list_workflows":
        _print_and_exit(_run_tt(action, tt_bin, ["run", "--list", "--json"], expect_json=True))
    if action == "plan_workflow":
        cmd = ["run", args.workflow, "--dry-run", "--json"]
        if args.agent:
            cmd.extend(["--agent", args.agent])
        if args.strict:
            cmd.append("--strict")
        _print_and_exit(_run_tt(action, tt_bin, cmd, expect_json=True))
    if action == "run_workflow":
        cmd = ["run", args.workflow, "--json"]
        if args.agent:
            cmd.extend(["--agent", args.agent])
        if args.strict:
            cmd.append("--strict")
        _print_and_exit(_run_tt(action, tt_bin, cmd, expect_json=True))
    if action == "verify_team":
        cmd = ["verify", "--json"]
        if args.workflow:
            cmd.extend(["--workflow", args.workflow])
        if args.agent:
            cmd.extend(["--agent", args.agent])
        if args.strict:
            cmd.append("--strict")
        _print_and_exit(_run_tt(action, tt_bin, cmd, expect_json=True))
    if action == "read_verify_status":
        _print_and_exit(
            _run_tt(action, tt_bin, ["verify", "--last", "--json"], expect_json=True)
        )
    if action == "generate_handoff":
        cmd = ["handoff", "generate", args.agent, "--json"]
        if args.reason:
            cmd.extend(["--reason", args.reason])
        if args.ctx is not None:
            cmd.extend(["--ctx", str(args.ctx)])
        _print_and_exit(_run_tt(action, tt_bin, cmd, expect_json=True))
    if action == "apply_handoff":
        cmd = ["handoff", "apply", args.agent]
        if args.packet:
            cmd.extend(["--packet", args.packet])
        _print_and_exit(_run_tt(action, tt_bin, cmd, expect_json=False))
    if action == "list_handoffs":
        cmd = ["handoff", "list", "--json", "--limit", str(args.limit)]
        if args.agent:
            cmd.extend(["--agent", args.agent])
        _print_and_exit(_run_tt(action, tt_bin, cmd, expect_json=True))
    if action == "team_status":
        _print_and_exit(_team_status(action))
    if action == "agent_output":
        _print_and_exit(_agent_output(action, tt_bin, args.agent, args.lines))
    if action == "send_prompt":
        if not args.prompt:
            result = CommandResult(
                ok=False,
                action=action,
                command=None,
                exit_code=2,
                data=None,
                stdout="",
                stderr="send_prompt requires a prompt",
            )
            _print_and_exit(result)
        cmd = ["send", args.agent]
        if args.auto_up:
            cmd.append("--auto-up")
        if args.wait:
            cmd.append("--wait")
            cmd.extend(["--timeout-secs", str(args.timeout_secs)])
            cmd.extend(["--idle-stable-secs", str(args.idle_stable_secs)])
        if args.output:
            cmd.append("--output")
            cmd.extend(["--output-lines", str(args.output_lines)])
        cmd.extend(args.prompt)
        _print_and_exit(_run_tt(action, tt_bin, cmd, expect_json=False))
    if action == "land_agent":
        cmd = ["land", args.agent]
        if args.pr:
            cmd.append("--pr")
        if args.force:
            cmd.append("--force")
        _print_and_exit(_run_tt(action, tt_bin, cmd, expect_json=False))
    if action == "stop_agent":
        _print_and_exit(_run_tt(action, tt_bin, ["down", args.agent], expect_json=False))
    if action == "stop_team":
        _print_and_exit(_run_tt(action, tt_bin, ["down"], expect_json=False))

    result = CommandResult(
        ok=False,
        action=action,
        command=None,
        exit_code=2,
        data=None,
        stdout="",
        stderr=f"unsupported action: {action}",
    )
    _print_and_exit(result)


if __name__ == "__main__":
    main()
