// Outline "Ballast" in Outfit and build the wordmark and lockups around the porthole symbol.
// Usage: node lockups.cjs <fontwork dir> <symbol.svg> <weight> <out dir>
const fs = require("fs");
const path = require("path");
const [fontwork, symbolFile, weight, outDir] = process.argv.slice(2);
const opentype = require(path.join(fontwork, "node_modules", "opentype.js"));

const fontFile = path.join(fontwork, "node_modules/@fontsource/outfit/files", `outfit-latin-${weight}-normal.woff`);
const buf = fs.readFileSync(fontFile);
const font = opentype.parse(buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength));

// Size the type so its cap height is 128 units: half the 256 symbol box.
const CAP = 128;
const capRatio = font.tables.os2.sCapHeight / font.unitsPerEm;
const size = CAP / capRatio;

// Lay the glyphs out by hand (glyph outlines plus the font's kerning). opentype.js 2.0's text
// shaping crashes on one of Outfit's substitution tables, and "Ballast" needs no substitutions.
function wordPath(x, baseline, sz = size) {
  const glyphs = [..."Ballast"].map((c) => font.charToGlyph(c));
  const scale = sz / font.unitsPerEm;
  const p = new opentype.Path();
  let pen = x;
  glyphs.forEach((g, i) => {
    p.extend(g.getPath(pen, baseline, sz));
    pen += g.advanceWidth * scale;
    if (i < glyphs.length - 1) pen += font.getKerningValue(g, glyphs[i + 1]) * scale;
  });
  return { d: p.toPathData(2), box: p.getBoundingBox() };
}

const symbolInner = fs.readFileSync(symbolFile, "utf8").match(/<g id="symbol">[\s\S]*?<\/g>/)[0];
const title = '<title id="t">Ballast</title>';
const open = (w, h) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${w} ${h}" role="img" aria-labelledby="t">${title}`;
const PAD = 16; // the symbol's own padding inside its 256 box
const write = (name, svg) => fs.writeFileSync(path.join(outDir, name), svg + "\n");
fs.mkdirSync(outDir, { recursive: true });

// Wordmark alone: cap height 128, padded like the symbol.
{
  const probe = wordPath(0, 0).box;
  const x = PAD - probe.x1;
  const baseline = PAD + CAP;
  const { d, box } = wordPath(x, baseline);
  const w = Math.ceil(box.x2 + PAD);
  const h = Math.ceil(baseline + PAD);
  write("wordmark.svg", `${open(w, h)}<!-- Outfit ${weight}, outlined with opentype.js. Cap height 128. -->\n<g id="wordmark"><path fill="currentColor" d="${d}"/></g></svg>`);
}

// Horizontal lockup: symbol, a gap of 48 (the large bubble's diameter plus a little), then the name.
// The capitals are centered on the symbol's middle (cap top 64, baseline 192).
{
  const GAP = 48;
  const probe = wordPath(0, 0).box;
  const x = 256 - PAD + GAP - probe.x1; // gap measured from the ring's outer edge
  const { d, box } = wordPath(x, 128 + CAP / 2);
  const w = Math.ceil(box.x2 + PAD);
  write("lockup-horizontal.svg", `${open(w, 256)}<!-- Symbol + Outfit ${weight} wordmark. Gap 48 from the ring; capitals centered on the symbol. -->\n${symbolInner}\n<g id="wordmark"><path fill="currentColor" d="${d}"/></g></svg>`);
}

// Stacked lockup: symbol above, the name centered below, smaller (cap height 80) so the symbol leads.
{
  const GAP = 40, CAP_S = 80, sz = size * CAP_S / CAP;
  const probe = wordPath(0, 0, sz).box;
  const textW = probe.x2 - probe.x1;
  const w = Math.ceil(Math.max(256, textW + 2 * PAD));
  const symX = (w - 256) / 2;
  const baseline = 256 - PAD + GAP + CAP_S;
  const { d } = wordPath((w - textW) / 2 - probe.x1, baseline, sz);
  const h = Math.ceil(baseline + PAD);
  write("lockup-stacked.svg", `${open(w, h)}<!-- Symbol above, Outfit ${weight} wordmark below (cap height 80), gap 40. -->
<g transform="translate(${symX.toFixed(2)} 0)">${symbolInner}</g>
<g id="wordmark"><path fill="currentColor" d="${d}"/></g></svg>`);
}
console.log(`Outfit ${weight}: size ${size.toFixed(1)}, cap ratio ${capRatio.toFixed(3)} -> ${outDir}`);
