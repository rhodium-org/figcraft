// Fork only, never offered upstream: proves the host extension points in the real wasm build
// before a release is published (host-release.yml). A bare page host, made here from the
// release's own index.html (data-host="page" on the canvas and a log of every figcraft:* event
// the app sends), drives each extension point in headless Chrome as a hosting page would.
//   npm ci && node prove.mjs <unzipped figcraft-web-<version>/> <out dir> [chrome]
import { createServer } from 'node:http';
import { readFile, stat, writeFile } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import puppeteer from 'puppeteer-core';

const [SITE, OUT, CHROME = '/usr/bin/google-chrome'] = process.argv.slice(2);

// The host page: the release's index.html, hosted in its own document, logging what the app sends.
const LOG = `<script>
window.__log = [];
{
  const c = document.getElementById('figcraft_canvas');
  for (const k of ['ready', 'save', 'dirty', 'failed', 'open-request', 'opened', 'documents']) c.addEventListener('figcraft:' + k, (e) => {
    const d = e.detail || {};
    window.__log.push({ kind: k, ...Object.fromEntries(Object.entries(d).filter(([n]) => n !== 'bytes')), bytes: d.bytes ? d.bytes.byteLength : undefined, at: performance.now() });
  });
  // A host may answer a save inside its own listener, before FigCraft's dispatch returns.
  c.addEventListener('figcraft:save', (e) => {
    const how = window.__answerAtOnce;
    if (!how || !e.detail || e.detail.id === undefined) return;
    c.dispatchEvent(new CustomEvent('figcraft:' + how, { detail: how === 'saved' ? { id: e.detail.id } : { id: e.detail.id, message: 'Refused at once.' } }));
  });
}
</script>
`;
const index = await readFile(join(SITE, 'index.html'), 'utf8');
if (!index.includes('<canvas id="figcraft_canvas"></canvas>')) throw new Error('index.html has no plain figcraft_canvas to host');
await writeFile(join(SITE, 'host.html'), index.replace('<canvas id="figcraft_canvas"></canvas>', '<canvas id="figcraft_canvas" data-host="page"></canvas>').replace('</body>', LOG + '</body>'));
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.css': 'text/css' };
const FRAME = 'Home from the host';

const server = createServer(async (req, res) => {
  const path = decodeURIComponent(new URL(req.url, 'http://x').pathname);
  const file = normalize(join(SITE, path));
  try { await stat(file); res.writeHead(200, { 'Content-Type': TYPES[extname(file)] ?? 'application/octet-stream' }).end(await readFile(file)); }
  catch { res.writeHead(404).end(); }
});
await new Promise((ok) => server.listen(0, '127.0.0.1', ok));
const browser = await puppeteer.launch({ executablePath: CHROME, headless: true,
  args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--ignore-gpu-blocklist', '--no-sandbox'] });

const results = [];
let failed = false;
const check = (name, ok, evidence) => {
  results.push({ name, ok, evidence });
  if (!ok) failed = true;
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}\n      ${JSON.stringify(evidence)}`);
};

try {
  const page = await browser.newPage();
  await page.setViewport({ width: 1400, height: 900 });
  page.on('console', (m) => { if (m.type() === 'error') console.log('page:', m.text()); });
  // Every time the app asks the browser for a file: a file input clicked or put on the page.
  await page.evaluateOnNewDocument(() => {
    window.__filePickers = 0;
    const click = HTMLInputElement.prototype.click;
    HTMLInputElement.prototype.click = function () {
      if (this.type === 'file') window.__filePickers += 1;
      return click.call(this);
    };
    new MutationObserver((records) => {
      for (const r of records) for (const n of r.addedNodes) if (n instanceof Element && (n.matches('input[type=file]') || n.querySelector('input[type=file]'))) window.__filePickers += 1;
    }).observe(document, { childList: true, subtree: true });
  });
  // ?control: window.figcraft, the page command API the checks drive the app through.
  await page.goto(`http://127.0.0.1:${server.address().port}/host.html?control&webgl`);
  await page.waitForFunction(() => window.__log.some((e) => e.kind === 'ready') && typeof window.figcraft === 'object', { timeout: 180_000 });

  await page.evaluate(() => {
    const c = document.getElementById('figcraft_canvas');
    window.send = (k, d) => c.dispatchEvent(new CustomEvent('figcraft:' + k, { detail: d }));
    window.call = (p) => p.then((v) => ({ ok: v }), (e) => ({ err: String(e) }));
    window.since = (t) => window.__log.filter((e) => e.at > t);
  });
  const now = () => page.evaluate(() => performance.now());
  const exec = (id, p = {}) => page.evaluate((i, q) => window.call(window.figcraft.execute(i, q)), id, p);
  const menu = (id) => page.evaluate((i) => window.call(window.figcraft.request('ui.menu.invoke', { id: i })), id);
  const doc = () => page.evaluate(() => window.call(window.figcraft.request('document.inspect', {})));
  const send = (k, d) => page.evaluate((a, b) => window.send(a, b), k, d);
  const since = (t) => page.evaluate((x) => window.since(x), t);
  const pickers = () => page.evaluate(() => window.__filePickers);
  const pause = (ms) => new Promise((ok) => setTimeout(ok, ms));
  const edit = (x) => exec('shape.rectangle', { x, y: 0, width: 40, height: 40 });

  // A design made by the app itself, handed back to it by the host as a file.
  await exec('file.new', { name: 'Poster' });
  await exec('node.create', { type: 'frame', x: 0, y: 0, width: 320, height: 200, name: FRAME });
  const made = await exec('file.saveAs', { data: true });
  check('the app makes a design file to host', typeof made.ok?.data === 'string', { name: made.ok?.name, err: made.err });
  await exec('file.close', { force: true });

  // ── Places: File ▸ Open belongs to a host that declares any ─────────────────────────
  let t = await now();
  let before = await pickers();
  await menu('file.open');
  await pause(800);
  check('without places, File ▸ Open is the browser\'s picker and asks the host nothing',
    (await since(t)).every((e) => e.kind !== 'open-request') && (await pickers()) > before, { log: await since(t), pickers: await pickers() });
  await page.keyboard.press('Escape');

  await send('places', { places: [{ id: 'computer', label: 'This computer' }, { id: 'github', label: 'GitHub' }] });
  await pause(300);
  t = await now();
  before = await pickers();
  await menu('file.open');
  await pause(800);
  const requests = (await since(t)).filter((e) => e.kind === 'open-request');
  check('with places, File ▸ Open sends figcraft:open-request and asks the browser for no file', requests.length === 1 && (await pickers()) === before, { requests, pickers: await pickers(), before });

  // The host answers with the design.
  await page.evaluate((b64) => window.send('open', { name: 'Poster.figcraft', bytes: Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)).buffer }), made.ok.data);
  let shown = '';
  for (let i = 0; i < 40 && !shown.includes(FRAME); i++) { await pause(500); shown = JSON.stringify((await doc()).ok ?? ''); }
  check('the design the host sends opens', shown.includes(FRAME), shown.slice(0, 300));
  await page.screenshot({ path: join(OUT, '1-opened-from-host.png') });

  await send('places', { places: [] });
  await pause(300);
  t = await now();
  before = await pickers();
  await menu('file.open');
  await pause(800);
  check('an empty places list gives File ▸ Open back to the browser', (await since(t)).every((e) => e.kind !== 'open-request') && (await pickers()) > before, await since(t));
  await page.keyboard.press('Escape');

  // ── Confirmed saves ──────────────────────────────────────────────────────────────────
  await edit(400);
  t = await now();
  await menu('file.save');
  await pause(500);
  let saves = (await since(t)).filter((e) => e.kind === 'save');
  check('before confirm-saves a Save carries no id and counts at once (unchanged)',
    saves.length === 1 && saves[0].id === undefined && saves[0].name === 'Poster.figcraft' && (await doc()).ok?.dirty === false, { saves, dirty: (await doc()).ok?.dirty });

  await send('confirm-saves', { value: true });
  await edit(500);
  t = await now();
  await menu('file.save');
  await pause(500);
  saves = (await since(t)).filter((e) => e.kind === 'save');
  const id1 = saves[0]?.id;
  check('after confirm-saves a Save carries an id and the design stays unsaved',
    saves.length === 1 && typeof id1 === 'string' && (await doc()).ok?.dirty === true, { saves, dirty: (await doc()).ok?.dirty });
  check('while a save waits, the host never hears the design is clean', !(await since(t)).some((e) => e.kind === 'dirty' && e.dirty === false), await since(t));

  t = await now();
  await send('save-failed', { id: id1, message: 'GitHub holds a newer version, so nothing was written.' });
  await pause(800);
  const lastError = (await menu('view.zoomFit')).ok?.lastError ?? '';
  check('figcraft:save-failed leaves the design unsaved and shows the host\'s reason',
    (await doc()).ok?.dirty === true && !(await since(t)).some((e) => e.kind === 'dirty' && e.dirty === false) && /newer version/.test(lastError), { lastError, dirty: (await doc()).ok?.dirty });
  await page.screenshot({ path: join(OUT, '2-save-failed.png') });

  t = await now();
  await menu('file.save');
  await pause(500);
  const id2 = (await since(t)).find((e) => e.kind === 'save')?.id;
  await send('saved', { id: id2 });
  await pause(800);
  check('figcraft:saved {id} counts the save: the design is clean and the host hears so',
    typeof id2 === 'string' && id2 !== id1 && (await doc()).ok?.dirty === false && (await since(t)).some((e) => e.kind === 'dirty' && e.dirty === false), { id2, log: await since(t) });

  await edit(600);
  t = await now();
  await menu('file.save');
  await pause(500);
  const id3 = (await since(t)).find((e) => e.kind === 'save')?.id;
  await edit(700);
  await send('saved', { id: id3 });
  await pause(800);
  check('an edit made while a save was on its way stays unsaved after it lands', typeof id3 === 'string' && (await doc()).ok?.dirty === true, { id3, dirty: (await doc()).ok?.dirty });

  t = await now();
  await send('saved', { id: '9999' });
  await send('save-failed', { id: 'junk' });
  await pause(500);
  check('answers for no save change nothing', (await doc()).ok?.dirty === true, await since(t));

  t = await now();
  await menu('file.saveAs');
  await pause(500);
  saves = (await since(t)).filter((e) => e.kind === 'save');
  // saveAs follows upstream's rule (hostmode::save_as): under the name of the host's own file it is a Save, false.
  check('Save As carries an id, and saveAs by upstream\'s name rule', saves.length === 1 && saves[0].saveAs === false && saves[0].name === 'Poster.figcraft' && typeof saves[0].id === 'string', saves);
  if (saves[0]?.id) await send('saved', { id: saves[0].id });
  await pause(500);
  check('a stored Save As leaves the design clean', (await doc()).ok?.dirty === false, (await doc()).ok?.dirty);

  // ── close-all: one design at a time, the host having asked the person ───────────────
  // The edit is heard as unsaved first, so the host can then hear close-all make it clean.
  t = await now();
  await edit(800);
  await page.waitForFunction((x) => window.__log.some((e) => e.kind === 'dirty' && e.dirty === true && e.at > x), { timeout: 15_000 }, t);
  t = await now();
  await send('close-all', {});
  await page.evaluate((b64) => window.send('open', { name: 'Poster.figcraft', bytes: Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)).buffer }), made.ok.data);
  shown = '';
  for (let i = 0; i < 40 && !shown.includes(FRAME); i++) { await pause(500); shown = JSON.stringify((await doc()).ok ?? ''); }
  const open = await exec('doc.list');
  const docs = Array.isArray(open.ok) ? open.ok.length : open.ok?.docs?.length;
  check('figcraft:close-all closes every design, unsaved changes and all, before the next open',
    docs === 1 && (await doc()).ok?.dirty === false && (await since(t)).some((e) => e.kind === 'dirty' && e.dirty === false), { docs, open, log: await since(t) });
  await edit(900);
  await pause(500);
  check('an edit to the design opened after close-all is heard as unsaved', (await since(t)).filter((e) => e.kind === 'dirty').at(-1)?.dirty === true, await since(t));
  await page.screenshot({ path: join(OUT, '3-after-close-all.png') });

  // ── Tabs: which document each open became, the tabs, and saves by document ─────────
  await send('close-all', {});
  await pause(300);
  t = await now();
  const sendOpen = (name) => page.evaluate((n, b64) => window.send('open', { name: n, bytes: Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)).buffer }), name, made.ok.data);
  await sendOpen('Poster.figcraft');
  await page.waitForFunction((x) => window.__log.filter((e) => e.kind === 'opened' && e.at > x).length >= 1, { timeout: 30_000 }, t);
  await sendOpen('Poster.figcraft');
  await page.waitForFunction((x) => window.__log.filter((e) => e.kind === 'opened' && e.at > x).length >= 2, { timeout: 30_000 }, t);
  const opened = (await since(t)).filter((e) => e.kind === 'opened');
  check('figcraft:opened names the document each open became, in the host\'s order, two of one name apart',
    opened.length === 2 && opened[1].open === opened[0].open + 1 && typeof opened[0].doc === 'number' && typeof opened[1].doc === 'number' && opened[0].doc !== opened[1].doc && opened.every((o) => o.name === 'Poster.figcraft'), opened);
  await pause(500);
  let tabs = (await since(t)).filter((e) => e.kind === 'documents').at(-1);
  const [first, second] = opened.map((o) => o.doc);
  check('figcraft:documents lists both tabs, the second in front, neither unsaved',
    tabs?.front === second && tabs?.docs?.map((d) => d.doc).join() === `${first},${second}` && tabs.docs.every((d) => d.dirty === false), tabs);

  t = await now();
  await edit(1000);
  await pause(800);
  tabs = (await since(t)).filter((e) => e.kind === 'documents').at(-1);
  check('an edit marks only the tab in front unsaved in figcraft:documents',
    tabs?.docs?.find((d) => d.doc === second)?.dirty === true && tabs?.docs?.find((d) => d.doc === first)?.dirty === false, tabs);

  t = await now();
  await menu('file.save');
  await pause(500);
  saves = (await since(t)).filter((e) => e.kind === 'save');
  check('a Save carries the document it is a file of', saves.length === 1 && saves[0].doc === second && saves[0].saveAs === false && saves[0].name === 'Poster.figcraft', saves);
  if (saves[0]?.id) await send('saved', { id: saves[0].id });
  await pause(800);
  tabs = (await since(t)).filter((e) => e.kind === 'documents').at(-1);
  check('the stored save marks that tab saved in figcraft:documents', tabs?.docs?.find((d) => d.doc === second)?.dirty === false, tabs);

  t = await now();
  // Sent as a Blob, which FigCraft (it reads bytes only) refuses: a malformed open is dropped by the crate and has no number.
  await page.evaluate(() => window.send('open', { name: 'AsBlob.figcraft', file: new Blob([new Uint8Array([1, 2, 3])]) }));
  await pause(500);
  const refused = (await since(t)).filter((e) => e.kind === 'opened');
  check('an open the app cannot take still uses its number and says no document', refused.length === 1 && refused[0].doc === null && refused[0].open === opened[1].open + 1, refused);

  // ── A host that answers a save at once, inside its own listener ────────────────────
  await page.evaluate(() => { window.__answerAtOnce = 'save-failed'; });
  await edit(1050);
  await menu('file.save');
  await pause(800);
  const alive = await page.evaluate(() => window.figcraft.inspect().then((d) => d.dirty, (e) => 'gone: ' + e));
  check('an answer of save-failed given inside the save listener leaves FigCraft running and the design unsaved', alive === true, alive);
  await page.evaluate(() => { window.__answerAtOnce = 'saved'; });
  await menu('file.save');
  await pause(800);
  const clean = await page.evaluate(() => window.figcraft.inspect().then((d) => d.dirty, (e) => 'gone: ' + e));
  check('an answer of saved given inside the save listener counts the save', clean === false, clean);
  await page.evaluate(() => { window.__answerAtOnce = undefined; });

  // ── AutoSave through a host that writes in place ───────────────────────────────────
  await exec('pref.autoSave', { on: true });
  t = await now();
  await edit(1100);
  await pause(5000);
  check('without figcraft:autosave nothing is saved on its own', !(await since(t)).some((e) => e.kind === 'save'), await since(t));
  await send('autosave', { available: true });
  t = await now();
  await edit(1200);
  await page.waitForFunction((x) => window.__log.some((e) => e.kind === 'save' && e.at > x), { timeout: 30_000 }, t).catch(() => {});
  saves = (await since(t)).filter((e) => e.kind === 'save');
  check('with figcraft:autosave {available: true}, a change is handed over on its own, as auto: true, for its document',
    saves.length === 1 && saves[0].auto === true && saves[0].doc === second && saves[0].name === 'Poster.figcraft' && typeof saves[0].id === 'string', saves);
  if (saves[0]?.id) await send('saved', { id: saves[0].id });
  await pause(800);
  tabs = (await since(t)).filter((e) => e.kind === 'documents').at(-1);
  check('the AutoSave the host stored counts: the tab is saved', tabs?.docs?.find((d) => d.doc === second)?.dirty === false, tabs);
  await send('autosave', { available: false });
  await exec('pref.autoSave', { on: false });

  // ── document-place: the home screen's Recent files, opened again through the host ──
  await send('document-place', { doc: second, place: 'github', key: 'acme/designs@main:Poster.figcraft', label: 'acme/designs · main · Poster.figcraft' });
  const stored = await page.evaluate(() => JSON.parse(localStorage.getItem('figcraft.hostRecents') ?? '[]'));
  check('figcraft:document-place is remembered in the browser for Recents, named after the file the host opened',
    stored.length === 1 && stored[0].place === 'github' && stored[0].key === 'acme/designs@main:Poster.figcraft' && stored[0].name === 'Poster.figcraft', stored);
  await page.evaluate(() => window.figcraft.request('ui.set', { home: true }));
  await pause(1500);
  await page.screenshot({ path: join(OUT, '4-home-recents.png') });
  t = await now();
  // The row is found by its name on the home screen, as a person finds it.
  const row = await page.evaluate(() => window.figcraft.request('ui.inspect', {}).then((u) => JSON.stringify(u)));
  let reopen = [];
  for (let y = 120; y <= 860 && reopen.length === 0; y += 14) {
    await page.mouse.click(420, y);
    await pause(150);
    reopen = (await since(t)).filter((e) => e.kind === 'open-request' && e.key !== undefined);
    await page.evaluate(() => window.figcraft.request('ui.set', { home: true }));
  }
  check('choosing it in Recents sends figcraft:open-request {place, key}', reopen.length === 1 && reopen[0].place === 'github' && reopen[0].key === 'acme/designs@main:Poster.figcraft', { reopen, inspected: row.length });
} finally {
  await writeFile(join(OUT, 'results.json'), JSON.stringify(results, null, 1));
  await browser.close();
  server.close();
}
console.log(failed ? 'SOME CHECKS FAILED' : 'ALL CHECKS PASSED');
process.exit(failed ? 1 : 0);
