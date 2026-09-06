#!/usr/bin/env python3
"""Place versioned gnr8 machine findings using GitHub workflow commands.

This integration owns the protocol encoding; the CLI owns all report formats.
Only current locations can be anchored. Removed source has no current location.
"""

import argparse
import json
import os
import sys

ANNOTATION_LIMIT = 50  # gnr8's per-project budget, not a GitHub platform limit.


def encode_data(value):
    """Escape workflow-command data, percent first to preserve literal escape sequences."""
    return value.replace('%', '%25').replace('\r', '%0D').replace('\n', '%0A')


def encode_property(value):
    """Properties additionally delimit on colon and comma."""
    return encode_data(value).replace(':', '%3A').replace(',', '%2C')


def emit(report, project_dir, artifact, stream, advisory=False):
    if type(report['schema_version']) is not int or report['schema_version'] != 1:
        raise ValueError('unsupported report schema_version')
    emitted = 0
    unanchorable = 0
    capped = 0
    for change in report['changes']:
        kind = change['kind']
        if kind == 'doc_only':
            continue
        if kind == 'breaking':
            level = 'error' if change['gating'] and not advisory else 'warning'
        elif kind == 'additive':
            level = 'notice'
        else:
            raise ValueError('unknown change kind')
        # Rust omits absent Option fields from JSON; an absent location is unanchorable.
        file = change.get('file')
        line = change.get('line')
        if not file or type(line) is not int or line < 1:
            unanchorable += 1
            continue
        if emitted >= ANNOTATION_LIMIT:
            capped += 1
            continue
        # Provenance is module-relative. This is the documented join; no alternative path search.
        properties = [('file', os.path.normpath(os.path.join(project_dir, file))),
                      ('line', str(line))]
        span = change.get('span')
        if span is not None:
            end = span['end_line']
            if type(end) is not int or end < line:
                raise ValueError('invalid source span')
            properties.append(('endLine', str(end)))
        properties.append(('title', 'gnr8: ' + change['code']))
        encoded = ','.join(key + '=' + encode_property(value) for key, value in properties)
        print('::' + level + ' ' + encoded + '::' + encode_data(change['message']), file=stream)
        emitted += 1
    omitted = capped + unanchorable
    if omitted:
        message = (f'gnr8: {omitted} further findings not annotated ({unanchorable} unanchorable); '
                   f'see the job summary and the "{artifact}" artifact.')
        print('::notice::' + encode_data(message), file=stream)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--advisory', action='store_true',
                        help='emit every breaking finding as a warning')
    parser.add_argument('report')
    parser.add_argument('project_dir')
    parser.add_argument('artifact')
    args = parser.parse_args()
    try:
        with open(args.report, encoding='utf-8') as source:
            report = json.load(source)
        emit(report, args.project_dir, args.artifact, sys.stdout, advisory=args.advisory)
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        # Never print raw JSON or exception text containing analyzed source as a workflow command.
        print('gnr8 action: cannot emit API change annotations from report.json; '
              'expected a readable schema_version 1 report with current source locations', file=sys.stderr)
        return 2
    return 0


if __name__ == '__main__':
    sys.exit(main())
