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

test('map zoom scales both axes, anchors the pointer, fits all rows and restores detail', async ({page})=>{
 await open(page);
 const sc=page.locator('#chart-scroll'), box=await sc.boundingBox();
 const point={x:box.x+242+(box.width-242)*.45,y:box.y+33+(box.height-33)*.45};
 const coords=()=>sc.evaluate((el,p)=>{
   const rect=el.getBoundingClientRect();const zoom=parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--cycle-width'));
   const height=document.querySelector('.instruction-row').getBoundingClientRect().height;
   return {cycle:(el.scrollLeft+p.x-rect.x-242)/zoom,row:(el.scrollTop+p.y-rect.y-33)/height,zoom,height};
 },point);
 const before=await coords();
 await page.mouse.move(point.x,point.y);await page.keyboard.down('Control');await page.mouse.wheel(0,-120);await page.keyboard.up('Control');
 await expect.poll(async()=>(await coords()).zoom).toBeGreaterThan(before.zoom);
 const after=await coords();expect(after.height).toBeGreaterThan(before.height);
 expect(Math.abs(after.cycle-before.cycle)).toBeLessThan(.1);expect(Math.abs(after.row-before.row)).toBeLessThan(.1);
 await page.keyboard.down('Shift');await page.mouse.dblclick(point.x,point.y);await page.keyboard.up('Shift');
 const reduced=await coords();expect(reduced.zoom).toBeCloseTo(after.zoom/2,1);expect(reduced.height).toBeLessThan(after.height);
 // Double-click the same location back in; no fetch-alignment jump should occur.
 await page.mouse.dblclick(point.x,point.y);
 expect((await coords()).zoom).toBeCloseTo(after.zoom,1);
 await page.getByRole('button',{name:'Fit',exact:true}).click();
 const fit=await sc.evaluate(el=>({w:el.scrollWidth,cw:el.clientWidth,h:el.scrollHeight,ch:el.clientHeight,top:el.scrollTop,left:el.scrollLeft}));
 expect(fit.w-fit.cw).toBeLessThanOrEqual(2);expect(fit.h-fit.ch).toBeLessThanOrEqual(2);expect(fit.top).toBe(0);expect(fit.left).toBe(0);
 await expect(page.locator('body')).toHaveClass(/map-overview/);
 await page.getByLabel('Bookmark this view').click();await page.getByLabel('View name').fill('Pipeline map');await page.getByRole('button',{name:'Save view',exact:true}).click();
 const fitHeight=await page.locator('.instruction-row').first().evaluate(el=>el.getBoundingClientRect().height);
 await page.getByRole('button',{name:'⊙ Focus F'}).click();
 expect(await page.locator('.instruction-row').first().evaluate(el=>el.getBoundingClientRect().height)).toBeGreaterThan(20);
 await page.locator('[data-bookmark="0"]').click();
 expect(await page.locator('.instruction-row').first().evaluate(el=>el.getBoundingClientRect().height)).toBeCloseTo(fitHeight,1);
});

test('two-finger pinch zooms the pipeline and never creates a selection range', async ({page,context})=>{
 await open(page);
 const client=await context.newCDPSession(page);await client.send('Emulation.setTouchEmulationEnabled',{enabled:true,maxTouchPoints:2});
 const box=await page.locator('#chart-scroll').boundingBox();const x=box.x+242+(box.width-242)/2,y=box.y+33+(box.height-33)/2;
 const value=()=>page.locator('.instruction-row').first().evaluate(el=>el.getBoundingClientRect().height);
 const coordinate=()=>page.locator('#chart-scroll').evaluate((el,p)=>{
   const rect=el.getBoundingClientRect();
   const width=parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--cycle-width'));
   const height=document.querySelector('.instruction-row').getBoundingClientRect().height;
   return [(el.scrollLeft+p.x-rect.x-242)/width,(el.scrollTop+p.y-rect.y-33)/height];
 },{x,y});
 const beforeCoordinate=await coordinate();
 const before=await value();
 const point=(id,px,py)=>({id,x:px,y:py,radiusX:3,radiusY:3,force:1});
 await client.send('Input.dispatchTouchEvent',{type:'touchStart',touchPoints:[point(1,x-50,y),point(2,x+50,y)]});
 await client.send('Input.dispatchTouchEvent',{type:'touchMove',touchPoints:[point(1,x-85,y-20),point(2,x+85,y+20)]});
 await client.send('Input.dispatchTouchEvent',{type:'touchEnd',touchPoints:[]});
 await expect.poll(value).toBeGreaterThan(before*1.5);
 const afterCoordinate=await coordinate();for(let i=0;i<2;i++)expect(Math.abs(afterCoordinate[i]-beforeCoordinate[i])).toBeLessThan(.15);
 await expect(page.locator('#range-overlay')).toBeHidden();
 // A fresh one-finger pan works after the pinch ends.
 const sc=page.locator('#chart-scroll'),left=await sc.evaluate(el=>el.scrollLeft);
 await client.send('Input.dispatchTouchEvent',{type:'touchStart',touchPoints:[point(1,x,y)]});
 await client.send('Input.dispatchTouchEvent',{type:'touchMove',touchPoints:[point(1,x-60,y)]});
 await client.send('Input.dispatchTouchEvent',{type:'touchEnd',touchPoints:[]});
 expect(await sc.evaluate(el=>el.scrollLeft)).toBeGreaterThan(left);
});
