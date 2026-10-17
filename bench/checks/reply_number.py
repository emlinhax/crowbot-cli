#!/usr/bin/env python3
"""Exit 0 when the reply (FILE) holds the number EXPECTED, however it is written: 4574, 4,574,
4574.00 or $4,574.0 all count.

Usage: reply_number.py FILE EXPECTED
"""
import re
import sys

reply = open(sys.argv[1]).read()
want = float(sys.argv[2])
numbers = [float(n.replace(",", "")) for n in re.findall(r"\d[\d,]*(?:\.\d+)?", reply)]
sys.exit(0 if any(abs(n - want) < 0.005 for n in numbers) else f"no {sys.argv[2]} in the reply")
