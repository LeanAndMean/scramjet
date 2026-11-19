# /// script
# requires-python = ">=3.10"
# dependencies = ["pyte>=0.8", "pillow>=10"]
# ///
"""A scripted session's screens in every look and theme, for the user to compare the looks.

    uv run scripts/gallery.py OUTDIR [--scenario scripts/screens/signals.json]
                              [--looks classic,rails,...] [--themes scramjet-dark,...]
                              [--binary target/release/scramjet]

Each look and theme's shots are written by scripts/screens.py under OUTDIR/LOOK/THEME/; then
OUTDIR/LOOK-SHOT.png puts a shot's themes side by side, and OUTDIR/index.html shows them all,
a look to a section."""
import argparse
import concurrent.futures
import html
import json
import os
import subprocess
import sys

from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOOKS = ['panels', 'classic', 'rails', 'margin', 'cards']
THEMES = ['scramjet-dark', 'pi-light', 'terminal-colors']


def capture(options, look, theme):
    outdir = os.path.join(options.outdir, look, theme)
    command = ['uv', 'run', '-q', os.path.join(ROOT, 'scripts/screens.py'), options.scenario, outdir,
               '--binary', options.binary, '--set', 'look=' + look, '--set', 'theme=' + theme]
    run = subprocess.run(command, capture_output=True, text=True)
    if run.returncode:
        sys.exit('gallery: %s in %s failed:\n%s%s' % (look, theme, run.stdout[-3000:], run.stderr[-3000:]))


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('outdir')
    parser.add_argument('--scenario', default=os.path.join(ROOT, 'scripts/screens/signals.json'))
    parser.add_argument('--looks', default=','.join(LOOKS))
    parser.add_argument('--themes', default=','.join(THEMES))
    parser.add_argument('--binary', default=os.path.join(ROOT, 'target/release/scramjet'))
    options = parser.parse_args()
    looks, themes = options.looks.split(','), options.themes.split(',')
    shots = [step['shot'] for step in json.load(open(options.scenario))['steps'] if 'shot' in step]
    with concurrent.futures.ThreadPoolExecutor(4) as pool:
        for done in [pool.submit(capture, options, look, theme) for look in looks for theme in themes]:
            done.result()
    page = ['<!doctype html><meta charset="utf-8"><title>Looks</title>',
            '<style>body{background:#222;color:#ddd;font-family:sans-serif}img{max-width:100%}</style>',
            '<p>Themes left to right: %s.</p>' % html.escape(', '.join(themes))]
    for look in looks:
        page.append('<h2 id="%s">%s</h2>' % (look, look))
        for shot in shots:
            images = [Image.open(os.path.join(options.outdir, look, theme, shot + '.png')) for theme in themes]
            sheet = Image.new('RGB', (sum(image.width for image in images) + 8 * (len(images) - 1),
                                      max(image.height for image in images)), (34, 34, 34))
            left = 0
            for image in images:
                sheet.paste(image, (left, 0))
                left += image.width + 8
            name = '%s-%s.png' % (look, shot)
            sheet.save(os.path.join(options.outdir, name))
            page.append('<h3>%s: %s</h3><img src="%s" alt="%s %s">' % (look, shot, name, look, shot))
            print(os.path.join(options.outdir, name))
    with open(os.path.join(options.outdir, 'index.html'), 'w') as file:
        file.write('\n'.join(page) + '\n')


if __name__ == '__main__':
    main()
