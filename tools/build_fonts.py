"""Builds the client's bundled fonts: client/assets/fonts/*.ttf.

egui cannot use variable fonts or woff2, so static TTFs are cut from the sources:
Onest 400/500/600/700 and JetBrains Mono 400 (Latin + Cyrillic subsets), and Phosphor
regular + fill with only the icons the client uses.

Setup (once): python -m venv venv && venv/Scripts/pip install fonttools

Sources (into one folder, e.g. dl/):
  Onest.ttf            https://github.com/google/fonts/raw/main/ofl/onest/Onest%5Bwght%5D.ttf
  JBMono.ttf           https://github.com/google/fonts/raw/main/ofl/jetbrainsmono/JetBrainsMono%5Bwght%5D.ttf
  Phosphor.ttf         https://unpkg.com/@phosphor-icons/web@2.1.1/src/regular/Phosphor.ttf
  Phosphor-Fill.ttf    https://unpkg.com/@phosphor-icons/web@2.1.1/src/fill/Phosphor-Fill.ttf
  phosphor-regular.css https://unpkg.com/@phosphor-icons/web@2.1.1/src/regular/style.css
  phosphor-fill.css    https://unpkg.com/@phosphor-icons/web@2.1.1/src/fill/style.css

Run:
  venv/Scripts/python -I tools/build_fonts.py dl out "$REG" "$FILL"
then copy out/*.ttf to client/assets/fonts/ and take codepoints for new icons from
out/icons.txt into client/src/icons.rs.

Icons in the current build (add a name here and to the run):
  REG=x,star,gear-six,check,microphone-slash,caret-down,caret-up,trash,plus,microphone,keyboard,key,arrows-clockwise,warning-circle,sliders-horizontal,pencil-simple,speaker-high,prohibit,warning,speaker-simple-slash,sign-out,screencast,monitor-play,corners-out,chat-circle,app-window,monitor,users-three,user-minus,user-plus,sun,sign-in,plugs,moon,lock-key,info,headphones,hash,eye,eye-slash,cpu,arrow-counter-clockwise,shield-star,copy,at,paper-plane-right,circle-notch,magnifying-glass,caret-right,list,shield,circle-dashed,identification-badge,minus,arrow-square-out,cloud-slash,check-circle,infinity,calendar-blank,hourglass-medium,clock,terminal-window,user,bug,folder-open
  FILL=star,shield-warning,megaphone,warning-circle,stop,paper-plane-right,crown-simple,check-circle,x-circle,circle,shield-star,wrench,hammer,heart,music-note,game-controller,microphone,lightning,rocket-launch,lock-simple,arrow-circle-down
"""
import re, sys, os
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer
from fontTools import subset

dl, out = sys.argv[1], sys.argv[2]
os.makedirs(out, exist_ok=True)

# Latin, Latin-1, Latin Extended-A, Cyrillic, general punctuation, arrows, symbols used in UI.
TEXT_UNICODES = (list(range(0x20, 0x7F)) + list(range(0xA0, 0x180)) + list(range(0x400, 0x460))
                 + list(range(0x2010, 0x2070)) + [0x20BD, 0x2116, 0x2122, 0x2190, 0x2192, 0x2212, 0x00D7, 0x22C5, 0x2026])

def subset_font(font, unicodes, path):
    opts = subset.Options()
    opts.layout_features = ['kern', 'liga', 'calt', 'tnum', 'case']
    opts.name_IDs = ['*']
    opts.notdef_outline = True
    s = subset.Subsetter(opts)
    s.populate(unicodes=unicodes)
    s.subset(font)
    font.save(path)
    print(path, os.path.getsize(path))

for wght, name in [(400, 'Regular'), (500, 'Medium'), (600, 'SemiBold'), (700, 'Bold')]:
    f = instancer.instantiateVariableFont(TTFont(f'{dl}/Onest.ttf'), {'wght': wght})
    subset_font(f, TEXT_UNICODES, f'{out}/Onest-{name}.ttf')

f = instancer.instantiateVariableFont(TTFont(f'{dl}/JBMono.ttf'), {'wght': 400})
subset_font(f, TEXT_UNICODES, f'{out}/JetBrainsMono-Regular.ttf')

ICONS = sys.argv[3].split(',')
FILL = sys.argv[4].split(',')
def codepoints(css, names):
    res = {}
    for n in names:
        m = re.search(r'\.ph-' + re.escape(n) + r':before\s*\{\s*content:\s*"' + chr(92)*2 + r'([0-9a-f]+)"', css)
        if not m: sys.exit(f'icon not found: {n}')
        res[n] = int(m.group(1), 16)
    return res
reg = codepoints(open(f'{dl}/phosphor-regular.css').read(), ICONS)
fil = codepoints(open(f'{dl}/phosphor-fill.css').read(), FILL)
subset_font(TTFont(f'{dl}/Phosphor.ttf'), list(reg.values()), f'{out}/Phosphor-Subset.ttf')
subset_font(TTFont(f'{dl}/Phosphor-Fill.ttf'), list(fil.values()), f'{out}/Phosphor-Fill-Subset.ttf')
with open(f'{out}/icons.txt', 'w') as fh:
    for n, c in sorted(reg.items()): fh.write(f'regular {n} {c:04X}\n')
    for n, c in sorted(fil.items()): fh.write(f'fill {n} {c:04X}\n')
