#!/usr/bin/env python3
"""Checks a simulated MCP server's recorded state (the run's .mcp/<server>.json).

Usage: mcp_state.py FILE SELECTOR OP VALUE
  SELECTOR  <collection>[<field>=<text>][<field>~<text>]...<.field>
            `=` keeps items whose field reads exactly <text>, `~` those containing it (any case).
  OP        ==   the field reads exactly VALUE
            ~    the field contains VALUE, in any case
            has  the field is a list holding VALUE
Every selected item must pass, and at least one must be selected. Exit 0 when it holds.
"""
import json
import re
import sys


def text(value):
    if isinstance(value, list):
        return ", ".join(text(v) for v in value)
    if isinstance(value, float) and value.is_integer():
        return str(int(value))
    return str(value)


def main(path, selector, op, want):
    state = json.load(open(path))["collections"]
    m = re.fullmatch(r"(\w+)((?:\[[^\]]+\])*)\.(\w+)", selector)
    if not m:
        sys.exit(f"bad selector {selector}")
    items = state.get(m.group(1), [])
    for cond in re.findall(r"\[([^\]]+)\]", m.group(2)):
        field, kind, value = re.fullmatch(r"(\w+)([=~])(.*)", cond).groups()
        if kind == "=":
            items = [i for i in items if text(i.get(field, "")) == value]
        else:
            items = [i for i in items if value.lower() in text(i.get(field, "")).lower()]
    if not items:
        sys.exit(f"{selector}: nothing selected")
    field = m.group(3)
    for item in items:
        have = item.get(field)
        ok = {
            "==": lambda: text(have) == want,
            "~": lambda: want.lower() in text(have).lower(),
            "has": lambda: isinstance(have, list) and want in [text(v) for v in have],
        }[op]()
        if not ok:
            sys.exit(f"{selector} {op} {want}: got {have!r}")


if __name__ == "__main__":
    main(*sys.argv[1:5])
