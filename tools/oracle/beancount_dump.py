"""Test oracle: load each ledger with real Beancount and print balances and errors as JSON.

Runs in its own process and virtualenv. BranchBooks never imports or links Beancount code.
Usage: python -I beancount_dump.py FILE [FILE ...]
"""
import json
import sys
from decimal import Decimal

import beancount
from beancount import loader
from beancount.core import data


def dump(path):
    entries, errors, _options = loader.load_file(path)
    balances = {}
    for entry in entries:
        if not isinstance(entry, data.Transaction):
            continue
        for posting in entry.postings:
            units = posting.units
            if units is None or units.number is None:
                continue
            by_currency = balances.setdefault(posting.account, {})
            by_currency[units.currency] = by_currency.get(units.currency, Decimal(0)) + units.number
    return {
        "balances": {
            account: {currency: str(number) for currency, number in sorted(by_currency.items())}
            for account, by_currency in sorted(balances.items())
        },
        "errors": [
            {
                "file": (error.source or {}).get("filename"),
                "line": (error.source or {}).get("lineno"),
                "type": type(error).__name__,
                "message": str(error.message).strip(),
            }
            for error in errors
        ],
    }


def main(paths):
    out = {"beancount": beancount.__version__, "results": {path: dump(path) for path in paths}}
    json.dump(out, sys.stdout, indent=2)


if __name__ == "__main__":
    main(sys.argv[1:])
