import puppeteer from 'puppeteer';
import { resolve, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const htmlPath = resolve(__dirname, 'carousel-pdf-export.html');
const outPdf = resolve(__dirname, 'axon-linkedin-carousel.pdf');
// The HTML is designed at 540x675 CSS px (half-size for screen preview).
// Render at that CSS size with deviceScaleFactor:2 → actual pixels = 1080x1350.
// PDF page = 1080x1350 CSS px expressed in inches at 96dpi.
const CSS_W = 540;
const CSS_H = 675;
const DPR = 2;
const SLIDE_W = CSS_W * DPR;  // 1080 actual px
const SLIDE_H = CSS_H * DPR;  // 1350 actual px
const IN_W = SLIDE_W / 96;    // 11.25in
const IN_H = SLIDE_H / 96;    // 14.0625in

const browser = await puppeteer.launch({
  headless: true,
  args: ['--no-sandbox', '--disable-setuid-sandbox'],
});

const page = await browser.newPage();

// CSS viewport = 540px wide. DPR=2 means screenshots are 1080px wide — full res.
await page.setViewport({ width: CSS_W, height: CSS_H * 10, deviceScaleFactor: DPR });
await page.goto(`file://${htmlPath}`, { waitUntil: 'networkidle0' });

// Hide controls/hint; slides are already 540x675 in the HTML — no size overrides needed
await page.addStyleTag({ content: `
  .controls, .hint { display: none !important; }
  body { background: #0d1117 !important; margin: 0; padding: 0; }
  .slides { gap: 0 !important; padding: 0 !important; align-items: flex-start !important; }
  .slide { border-radius: 0 !important; flex-shrink: 0 !important; }
` });

const slideCount = await page.$$eval('.slide', els => els.length);
console.log(`Found ${slideCount} slides`);

const pngBuffers = [];

for (let i = 0; i < slideCount; i++) {
  const slide = (await page.$$('.slide'))[i];
  const box = await slide.boundingBox();

  const buf = await page.screenshot({
    type: 'png',
    clip: { x: box.x, y: box.y, width: box.width, height: box.height },
  });
  pngBuffers.push(buf);
  console.log(`  slide ${i + 1}/${slideCount} captured (${box.width}x${box.height})`);
}

await browser.close();

// Assemble PDF: each image is base64-embedded in an HTML page whose CSS page
// size matches the image pixel dimensions exactly (using pt units).
const pdfBrowser = await puppeteer.launch({ headless: true, args: ['--no-sandbox'] });
const pdfPage = await pdfBrowser.newPage();

const imgs = pngBuffers.map((buf, i) => {
  const b64 = buf.toString('base64');
  // Image is SLIDE_W x SLIDE_H px. Page is PT_W x PT_H pt.
  // Set img to 100% width/height so it fills the page with zero stretch.
  return `<div class="page"><img src="data:image/png;base64,${b64}"></div>`;
}).join('\n');

const html = `<!DOCTYPE html><html><head><style>
  * { margin:0; padding:0; box-sizing:border-box; }
  @page { size: ${IN_W}in ${IN_H}in; margin: 0; }
  body { background:#0d1117; }
  .page {
    width: ${IN_W}in;
    height: ${IN_H}in;
    overflow: hidden;
    page-break-after: always;
    break-after: page;
  }
  .page:last-child { page-break-after: avoid; break-after: avoid; }
  img {
    width: ${IN_W}in;
    height: ${IN_H}in;
    display: block;
    -webkit-print-color-adjust: exact;
    print-color-adjust: exact;
  }
</style></head><body>${imgs}</body></html>`;

await pdfPage.setContent(html, { waitUntil: 'networkidle0' });
await pdfPage.pdf({
  path: outPdf,
  width: `${IN_W}in`,
  height: `${IN_H}in`,
  printBackground: true,
  margin: { top: 0, right: 0, bottom: 0, left: 0 },
});

await pdfBrowser.close();
console.log(`\nPDF written: ${outPdf} (${IN_W}in x ${IN_H}in, 2x DPI images)`);
