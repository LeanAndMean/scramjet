#!/usr/bin/env python3
"""Fails when a private term is about to enter the repository.

The terms (names this repository must not hold) are regular expressions, one per line before a
tab, in the file $SCRAMJET_RS_PRIVATE_TERMS names, or ../scramjet-rs-private/terms.tsv beside the
repository. Without that file nothing is checked. The spelling dictionaries are left out.

  private-terms.py              the staged changes' added lines
  private-terms.py --message F  a commit message's file, as the commit-msg hook gives it
  private-terms.py --tree       every tracked file
  private-terms.py --history    the message and changes of every commit on a branch or tag
"""
import os, re, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TERMS = os.environ.get('SCRAMJET_RS_PRIVATE_TERMS') or os.path.join(os.path.dirname(ROOT), 'scramjet-rs-private', 'terms.tsv')
PATHS = ['--', '.', ':(exclude)src/agent_cli/spelling']
REFS = ['--branches', '--tags', '--remotes']


def git(*words):
    return subprocess.run(['git', '-C', ROOT] + list(words), capture_output=True, check=True).stdout


def texts(mode):
    if mode == '--message':
        yield 'the message', open(sys.argv[2], 'rb').read()
    elif mode == '--tree':
        yield 'tracked files', git('grep', '-I', '-n', '', *PATHS)
    elif mode == '--history':
        yield 'messages', git('log', *REFS, '--format=%h %B')
        yield 'changes', git('log', *REFS, '-p', '--format=commit %h', *PATHS)
    else:
        added = [line for line in git('diff', '--cached', '-U0', *PATHS).split(b'\n')
                 if line.startswith(b'+')]
        yield 'staged changes', b'\n'.join(added)


if not os.path.exists(TERMS):
    print('private-terms: no list at %s, nothing checked' % TERMS, file=sys.stderr)
    sys.exit(0)
rules = [re.compile(line.split('\t')[0].encode()) for line in open(TERMS, encoding='utf-8')
         if line.strip() and not line.startswith('#')]
found = 0
for where, text in texts(sys.argv[1] if len(sys.argv) > 1 else '--staged'):
    for line in text.split(b'\n'):
        if any(rule.search(line) for rule in rules):
            found += 1
            if found <= 20:
                print('private-terms: in %s: %s' % (where, line.decode('utf-8', 'replace')[:200]), file=sys.stderr)
if found:
    print('private-terms: %d lines hold a private term (%s)' % (found, TERMS), file=sys.stderr)
    sys.exit(1)
