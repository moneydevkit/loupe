"""Render a local, self-contained HTML report of a repo's loupe findings.

Reads through loupectl (so it needs the admin bundle: run on the box as
root, or via `just report`) and writes a single HTML file. Nothing leaves
the machine.

Usage:
    loupe-report [REPO_ID] [-n LIMIT] [-o OUT]

`loupectl finding list` has no --json, so ids come from its text output and
each detail is fetched with `finding show <id> --json`. Verifier verdict
notes are deliberately absent: the server exposes finding_verifications
only via EXISTS checks, so the "why inconclusive" reason is in the DB but
not reachable through any API or CLI.
"""

import argparse
import datetime
import html
import json
import os
import pwd
import subprocess
import sys

STATE_ORDER = ["validating", "awaiting_approval", "confirmed", "dismissed"]
STATE_BLURB = {
    "validating": (
        "Verifier returned inconclusive (non-terminal), so the state machine "
        "left these parked. The server reaper auto-dismisses stale validating "
        "findings once their deadline elapses (budget: 7 days). Triage or "
        "`loupectl finding retry-verify` before then."
    ),
    "awaiting_approval": "Verifier confirmed; parked for human sign-off.",
    "confirmed": "Scanner and verifier agree. Highest-signal set.",
    "dismissed": "Verifier dismissed, or reaped past deadline.",
}
SEVERITY_ORDER = {"critical": 0, "high": 1, "medium": 2, "low": 3, "info": 4}


def loupectl(args):
    return subprocess.run(
        ["loupectl", *args], check=True, capture_output=True, text=True
    ).stdout


def finding_ids(repo_id, limit):
    out = loupectl(["finding", "list", str(repo_id), "-n", str(limit)])
    ids = []
    for line in out.splitlines():
        head = line.split()[:1]
        if head and head[0].isdigit():
            ids.append(int(head[0]))
    return sorted(set(ids))


def ts(v):
    if not v:
        return ""
    return datetime.datetime.fromtimestamp(v).strftime("%Y-%m-%d %H:%M")


def block(title, body, lang=False):
    """Collapsible section; omitted entirely when body is empty."""
    if not body:
        return ""
    tag = "pre" if lang else "div"
    cls = ' class="code"' if lang else ' class="prose"'
    return (
        f"<details><summary>{html.escape(title)}</summary>"
        f"<{tag}{cls}>{html.escape(body)}</{tag}></details>"
    )


def render(repo_id, findings):
    by_state = {}
    for f in findings:
        by_state.setdefault(str(f.get("state", "unknown")), []).append(f)
    for rows in by_state.values():
        rows.sort(
            key=lambda f: (
                SEVERITY_ORDER.get(str(f.get("severity", "")).lower(), 9),
                f.get("id", 0),
            )
        )

    counts = " · ".join(
        f"{len(by_state[s])} {s.replace('_', ' ')}"
        for s in STATE_ORDER
        if by_state.get(s)
    )
    now = ts(int(__import__("time").time()))
    parts = [
        "<style>",
        ":root{color-scheme:dark light}",
        "body{font:15px/1.55 ui-sans-serif,system-ui,sans-serif;margin:0;"
        "padding:2rem 1.25rem 6rem;max-width:62rem;margin-inline:auto;"
        "background:#0f1115;color:#e6e6e6}",
        "h1{font-size:1.5rem;margin:0 0 .25rem}",
        ".sub{color:#8b93a7;font-size:.85rem;margin-bottom:2rem}",
        "h2{font-size:1.05rem;margin:2.5rem 0 .35rem;text-transform:uppercase;"
        "letter-spacing:.06em;color:#c9d1e0}",
        ".blurb{color:#8b93a7;font-size:.85rem;margin:0 0 1rem;max-width:52rem}",
        ".f{border:1px solid #262b36;border-radius:8px;padding:.9rem 1rem;"
        "margin-bottom:.7rem;background:#151821}",
        ".t{font-weight:600;display:flex;gap:.6rem;align-items:baseline;"
        "flex-wrap:wrap}",
        ".id{color:#5b6478;font-variant-numeric:tabular-nums;font-size:.85rem}",
        ".loc{font-family:ui-monospace,monospace;font-size:.8rem;color:#7f8aa3;"
        "margin-top:.3rem;word-break:break-all}",
        ".sev{font-size:.68rem;font-weight:700;padding:.15rem .45rem;"
        "border-radius:4px;text-transform:uppercase;letter-spacing:.04em}",
        ".critical,.high{background:#4c1d1d;color:#ffb4b4}",
        ".medium{background:#4a3410;color:#ffd08a}",
        ".low,.info{background:#23313f;color:#9fc4e0}",
        "details{margin-top:.55rem}",
        "summary{cursor:pointer;font-size:.82rem;color:#8fa8c8}",
        ".prose{white-space:pre-wrap;margin:.5rem 0 0;font-size:.9rem;"
        "color:#cdd3df}",
        ".code{overflow-x:auto;background:#0b0d12;border:1px solid #222733;"
        "border-radius:6px;padding:.7rem;font-size:.78rem;"
        "font-family:ui-monospace,monospace;margin:.5rem 0 0}",
        "</style>",
        f"<h1>loupe findings · repo {html.escape(str(repo_id))}</h1>",
        f'<div class="sub">{html.escape(counts)} · generated {now} '
        "· local file, nothing uploaded</div>",
    ]

    for state in STATE_ORDER + sorted(set(by_state) - set(STATE_ORDER)):
        rows = by_state.get(state)
        if not rows:
            continue
        parts.append(
            f"<h2>{html.escape(state.replace('_', ' '))} ({len(rows)})</h2>"
        )
        if STATE_BLURB.get(state):
            parts.append(f'<p class="blurb">{html.escape(STATE_BLURB[state])}</p>')
        for f in rows:
            sev = str(f.get("severity", "")).lower()
            loc = f.get("file_path") or ""
            if loc and f.get("line_start"):
                loc += f":{f['line_start']}"
                if f.get("line_end") and f["line_end"] != f["line_start"]:
                    loc += f"-{f['line_end']}"
            meta = [f"created {ts(f.get('created_at'))}"]
            if f.get("cwe"):
                meta.append(str(f["cwe"]))
            if f.get("scanner_id"):
                meta.append(str(f["scanner_id"]))
            if f.get("approved_by_cn"):
                meta.append(f"approved by {f['approved_by_cn']}")
            if f.get("rejected_by_cn"):
                meta.append(f"rejected by {f['rejected_by_cn']}")
            parts += [
                '<div class="f">',
                f'<div class="t"><span class="id">#{f.get("id")}</span>'
                f'<span class="sev {html.escape(sev)}">{html.escape(sev)}</span>'
                f'<span>{html.escape(str(f.get("title", "")))}</span></div>',
                f'<div class="loc">{html.escape(loc)} · '
                f'{html.escape(" · ".join(meta))}</div>',
                block("description", f.get("description", "")),
                block("proof of concept (diff)", f.get("poc_unified") or "", lang=True),
                block("proposed patch (advisory)", f.get("patch_unified") or "", lang=True),
                "</div>",
            ]
    return (
        "<!doctype html><meta charset=utf-8><title>loupe findings</title>"
        + "".join(parts)
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("repo_id", nargs="?", default="1")
    ap.add_argument("-n", "--limit", default=500, type=int)
    ap.add_argument("-o", "--out")
    args = ap.parse_args()

    invoker = os.environ.get("SUDO_USER")
    if args.out:
        out = args.out
    elif invoker:
        out = os.path.join(pwd.getpwnam(invoker).pw_dir, "loupe-findings.html")
    else:
        out = os.path.expanduser("~/loupe-findings.html")

    ids = finding_ids(args.repo_id, args.limit)
    if not ids:
        sys.exit(f"no findings listed for repo {args.repo_id}")
    findings = []
    for i in ids:
        try:
            findings.append(
                json.loads(loupectl(["finding", "show", str(i), "--json"]))
            )
        except (subprocess.CalledProcessError, json.JSONDecodeError) as e:
            print(f"warning: skipping finding {i}: {e}", file=sys.stderr)

    with open(out, "w") as fh:
        fh.write(render(args.repo_id, findings))
    if invoker:  # written as root; hand it back to the human
        info = pwd.getpwnam(invoker)
        os.chown(out, info.pw_uid, info.pw_gid)
    os.chmod(out, 0o600)
    print(f"wrote {out} ({len(findings)} findings)")


if __name__ == "__main__":
    main()
