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
  for (const k of ['ready', 'save', 'dirty', 'failed', 'open-request']) c.addEventListener('figcraft:' + k, (e) => {
    const d = e.detail || {};
    window.__log.push({ kind: k, ...Object.fromEntries(Object.entries(d).filter(([n]) => n !== 'bytes')), bytes: d.bytes ? d.bytes.byteLength : undefined, at: performance.now() });
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
  check('Save As carries saveAs and an id', saves.length === 1 && saves[0].saveAs === true && typeof saves[0].id === 'string', saves);
  if (saves[0]?.id) await send('saved', { id: saves[0].id });
  await pause(500);
  check('a stored Save As leaves the design clean', (await doc()).ok?.dirty === false, (await doc()).ok?.dirty);
} finally {
  await writeFile(join(OUT, 'results.json'), JSON.stringify(results, null, 1));
  await browser.close();
  server.close();
}
console.log(failed ? 'SOME CHECKS FAILED' : 'ALL CHECKS PASSED');
process.exit(failed ? 1 : 0);
