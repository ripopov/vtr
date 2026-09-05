const { test, expect } = require('@playwright/test');
const path = require('path');
const fs = require('fs');
const shots = path.join(__dirname, '../screenshots');
async function open(page) { await page.goto('/'); await expect(page.locator('.instruction-row')).toHaveCount(72); }
async function example(page, id) {
  for (const dialog of await page.locator('dialog[open]').all()) await dialog.press('Escape');
  await page.getByRole('tab', {name:'Field guide'}).click();
  await page.locator(`#lesson-${id} [data-lesson]`).click();
}
async function screenshot(page,id) {
  if(!process.env.CAPTURE) return;
  // Wait for font/layout completion and the transient status message to settle.
  await page.waitForTimeout(2800);
  await page.screenshot({path:path.join(shots,id+'.png'),animations:'disabled'});
}
test('pipeline selection, causal navigation, filtering, measurement, and comparison', async ({page}) => {
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await open(page);
  const retirement=await page.locator('.stage[aria-label*=", Retire,"]').evaluateAll(items=>items.map(e=>Number(e.getAttribute('aria-label').match(/to (\d+)/)[1])));
  expect(retirement).toEqual([...retirement].sort((a,b)=>a-b));
  await expect(page.locator('.instruction-title')).toHaveText('ld a0, 0(s1)');
  await expect(page.locator('#inspector')).toContainText('18 cycles');
  await page.locator('[data-row="1013"] .row-label').click();
  await expect(page.locator('#inspector')).toContainText('Waiting for a0');
  await page.locator('#inspector [data-jump="1012"]').click();
  await expect(page.locator('[data-row="1012"]')).toHaveClass(/selected/);
  await expect(page.locator('#arrows > path')).toHaveCount(2);
  await page.getByLabel('Dependency arrows').uncheck();
  await expect(page.locator('#arrows > path')).toHaveCount(0);
  await page.getByLabel('Dependency arrows').check();
  await page.locator('[data-row="1012"] .stage[data-cycle="23"]').click();
  await expect(page.locator('#cursor-tag')).toHaveText('23');
  await page.locator('[data-row="1012"] .row-label').click();
  await page.keyboard.press('ArrowDown');await page.keyboard.press('ArrowDown');
  await expect(page.locator('[data-row="1014"]')).toHaveClass(/selected/);
  await page.locator('#inspector [data-jump="1012"]').click();
  await page.getByLabel('Search instructions').fill('ld');
  await expect(page.locator('.instruction-row')).toHaveCount(17);
  await expect(page.locator('[data-row="1012"]')).toBeInViewport();
  await page.getByLabel('Search instructions').fill('no-matching-instruction');
  await expect(page.getByRole('heading',{name:'No instructions match'})).toBeVisible();
  await page.getByRole('button',{name:'Clear filters',exact:true}).click();
  await expect(page.locator('.instruction-row')).toHaveCount(72);
  await page.getByLabel('Range start cycle').fill('23');await page.getByLabel('Range end cycle').fill('41');await page.getByRole('button',{name:'Apply',exact:true}).click();
  await expect(page.locator('#range-overlay')).toBeVisible();
  await expect(page.locator('.metric').first()).toContainText('18');
  const metrics=await page.locator('#analysis-bar').innerText();
  await page.locator('[data-filter="load"]').click();
  expect(await page.locator('#analysis-bar').innerText()).toEqual(metrics);
  await page.getByRole('button',{name:'Compare',exact:true}).click();
  await expect(page.locator('.compare-delta')).toContainText('8 cycles earlier');
  await expect(page.locator('.ghost-stage').first()).toBeAttached();
  await page.getByLabel('Clear measurement').click();
  await expect(page.locator('#range-overlay')).toBeHidden();
  await page.getByLabel('Range start cycle').fill('50');await page.getByLabel('Range end cycle').fill('40');await page.getByRole('button',{name:'Apply',exact:true}).click();
  await expect(page.locator('#toast')).toContainText('end after the start');
  expect(errors).toEqual([]);
});
test('keyboard, pointer pan/zoom, overview, range dragging, and dialogs', async ({page})=>{
  await open(page);
  const sc=page.locator('#chart-scroll');await sc.focus();
  await page.keyboard.press('ArrowDown');await expect(page.locator('[data-row="1013"]')).toHaveClass(/selected/);
  await page.keyboard.press('ArrowRight');await expect(page.locator('#cursor-tag')).toHaveText('28');
  const before=await page.locator('#zoom-label').textContent();await page.keyboard.press('+');expect(await page.locator('#zoom-label').textContent()).not.toEqual(before);
  await page.keyboard.press('f');
  const box=await sc.boundingBox();const x=box.x+300,y=box.y+100;
  const left=await sc.evaluate(e=>e.scrollLeft);
  await page.mouse.move(x+150,y);await page.mouse.down();await page.mouse.move(x+50,y,{steps:6});await page.mouse.up();
  expect(await sc.evaluate(e=>e.scrollLeft)).toBeGreaterThan(left);
  await page.getByRole('button',{name:'Measure',exact:true}).click();
  await page.mouse.move(x+10,y);await page.mouse.down();await page.mouse.move(x+150,y,{steps:6});await page.mouse.up();
  await expect(page.locator('#range-overlay')).toBeVisible();
  const ov=page.locator('#overview');await ov.focus();await page.keyboard.press('Home');expect(await sc.evaluate(e=>e.scrollLeft)).toBe(0);
  await page.keyboard.press('End');expect(await sc.evaluate(e=>e.scrollLeft)).toBeGreaterThan(100);
  await page.getByRole('button',{name:'Commands K'}).click();await expect(page.getByRole('dialog',{name:'Go anywhere. Do anything.'})).toBeVisible();
  await page.getByRole('combobox',{name:'Search commands'}).fill('#1056');await page.keyboard.press('Enter');await expect(page.locator('[data-row="1056"]')).toHaveClass(/selected/);
  await page.getByRole('button',{name:'Keyboard shortcuts',exact:true}).click();await expect(page.locator('#shortcuts')).toBeVisible();await page.keyboard.press('Escape');await expect(page.locator('#shortcuts')).toBeHidden();
  await page.getByRole('tab',{name:'Field guide'}).focus();await page.keyboard.press('ArrowRight');await expect(page.getByRole('tab',{name:'Pipeline viewer'})).toHaveAttribute('aria-selected','true');
});
test('saved views, pins, notes persist and export contains the current evidence', async ({page})=>{
 await open(page);
 await page.getByRole('button',{name:'◇ Pin',exact:true}).click();await expect(page.locator('#pins')).toContainText('#1012');
 await page.getByLabel('Private note for #1012 in this trace').fill('Memory latency delays two dependent instructions.');await page.getByRole('button',{name:'Save note',exact:true}).click();
 await page.getByLabel('Bookmark this view').click();await page.getByLabel('View name').fill('First memory stall');await page.getByRole('button',{name:'Save view',exact:true}).click();
 await expect(page.locator('#bookmarks')).toContainText('First memory stall');
 await page.getByLabel('Search instructions').fill('bne');await page.locator('[data-bookmark="0"]').click();await expect(page.getByLabel('Search instructions')).toHaveValue('');
 const downloadPromise=page.waitForEvent('download');await page.getByRole('button',{name:'Export view'}).click();const download=await downloadPromise;const report=JSON.parse(fs.readFileSync(await download.path(),'utf8'));expect(report.note).toBe('Memory latency delays two dependent instructions.');expect(report.instruction.id).toBe(1012);expect(report.view.selected).toBe(1012);
 await page.reload();await expect(page.locator('#pins')).toContainText('#1012');await expect(page.locator('#bookmarks')).toContainText('First memory stall');await expect(page.getByLabel('Private note for #1012 in this trace')).toHaveValue(report.note);
 await page.getByLabel('Delete saved view First memory stall').click();await page.getByLabel('Unpin instruction 1012').click();await expect(page.locator('#pins')).not.toContainText('#1012');
});
test('scenario switching, flush distinction, literal search, theme and narrow layouts', async ({page})=>{
 await open(page);await page.getByLabel('Trace',{exact:true}).selectOption('branch');await page.locator('[data-filter="flush"]').click();await expect(page.locator('.instruction-row')).toHaveCount(5);await expect(page.locator('.stage.flushed').first()).toBeVisible();
 await page.getByLabel('Show flushed').uncheck();await expect(page.locator('#empty')).toBeVisible();await page.getByRole('button',{name:'Clear filters',exact:true}).click();
 await page.getByLabel('Trace',{exact:true}).selectOption('steady');await page.locator('[data-filter="flush"]').click();await expect(page.locator('#empty')).toBeVisible();
 await page.getByRole('button',{name:'Clear filters',exact:true}).click();await page.getByLabel('Search instructions').fill('<script>');await expect(page.locator('#empty')).toBeVisible();
 await page.getByRole('button',{name:'Reset view',exact:true}).click();await page.getByLabel('Switch to dark theme').click();await expect(page.locator('body')).toHaveClass(/dark/);await page.getByLabel('Compact rows').check();await expect(page.locator('body')).toHaveClass(/compact/);
 for (const width of [1280,900,390]) {await page.setViewportSize({width,height:900});expect(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth)).toBeTruthy();await expect(page.getByRole('button',{name:'Export view'})).toBeVisible();}
});
test('tutorial covers every feature with working examples and full-size screenshots', async ({page})=>{
 test.setTimeout(120000);
 await open(page);await page.getByRole('tab',{name:'Field guide'}).click();await expect(page.locator('.chapter')).toHaveCount(12);
 const ids=['overview','navigate','inspect','dependencies','search','flush','measure','compare','bookmarks','notes','commands','appearance'];
 for(const id of ids){
  await example(page,id);
  if(id==='notes'){await page.getByLabel('Private note for #1012 in this trace').fill('The load waits 18 cycles for a memory response. Follow a0 to see the two delayed consumers.');await page.getByRole('button',{name:'Save note',exact:true}).click();}
  await screenshot(page,id);
  if(id==='search'){await page.getByLabel('Search instructions').fill('no-matching-instruction');await screenshot(page,'search-empty');}
  if(id==='bookmarks'){await page.getByRole('button',{name:'Save view',exact:true}).click();await screenshot(page,'saved-view');}
  if(id==='commands'){await page.keyboard.press('Escape');await page.getByRole('button',{name:'Keyboard shortcuts',exact:true}).click();await screenshot(page,'shortcuts');}
  if(id==='appearance'){await page.setViewportSize({width:900,height:1000});await screenshot(page,'narrow');await page.setViewportSize({width:1600,height:1000});}
 }
 await page.reload();await page.getByRole('tab',{name:'Field guide'}).click();
 if(process.env.CAPTURE){await page.waitForTimeout(200);await page.screenshot({path:path.join(shots,'tutorial.png'),animations:'disabled'});}
 for (const image of await page.locator('.chapter img').all()){await image.scrollIntoViewIfNeeded();await expect(image).toHaveJSProperty('complete',true);expect(await image.evaluate(e=>e.naturalWidth)).toBeGreaterThan(0);}
 await page.locator('#lesson-overview .screenshot-button').click();await expect(page.locator('#image-dialog')).toBeVisible();await page.getByLabel('Close screenshot').click();await expect(page.locator('#image-dialog')).toBeHidden();
});

test('standalone file opens without network or a server', async ({page})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.route(/^https?:/,route=>route.abort());
 await page.goto(require('url').pathToFileURL(path.join(__dirname,'../index.html')).href);
 await expect(page.locator('.instruction-row')).toHaveCount(72);
 await page.getByRole('button',{name:'Compare',exact:true}).click();
 await expect(page.locator('.compare-delta')).toContainText('8 cycles earlier');
 await page.getByRole('tab',{name:'Field guide'}).click();
 const image=page.locator('#lesson-overview img');await image.scrollIntoViewIfNeeded();
 await expect(image).toHaveJSProperty('complete',true);expect(await image.evaluate(e=>e.naturalWidth)).toBeGreaterThan(0);
 expect(errors).toEqual([]);
});
