// node --test docs/tests/table-baseline.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';
const html=await readFile(new URL('../table-baseline.html',import.meta.url),'utf8');
const routes={'/':{type:'text/html',body:html}};

for(const width of [1280,390]) test(`baseline spec layout and virtual rows at ${width}px`,{timeout:30000},async t=>{
  const browser=await browserTest(routes);t.after(()=>browser.close());
  await browser.send('Emulation.setDeviceMetricsOverride',{width,height:1000,deviceScaleFactor:1,mobile:false});
  await browser.wait('document.querySelectorAll("#rows tr").length === 12');
  assert.equal(await browser.evaluate('document.documentElement.scrollWidth <= innerWidth'),true,'no page overflow');
  assert.equal(await browser.evaluate('document.querySelectorAll("#rows td").length'),72);
  assert.equal(await browser.evaluate('document.querySelectorAll("#records th").length'),6);
  assert.equal(await browser.evaluate('document.querySelector("#records").getAttribute("aria-rowcount")'),'1000001');
  const inaccessible=await browser.evaluate(`[...document.querySelectorAll('button,input,select,textarea')].filter(e=>!e.textContent.trim()&&!e.getAttribute('aria-label')&&!e.title&&!document.querySelector('label[for="'+e.id+'"]')).map(e=>e.id)`);
  assert.deepEqual(inaccessible,[],'controls have names');
  const broken=await browser.evaluate(`[...document.querySelectorAll('a[href^="#"]')].filter(a=>!document.getElementById(a.hash.slice(1))).map(a=>a.hash)`);
  assert.deepEqual(broken,[],'in-page navigation targets exist');
  await browser.click('#theme');assert.equal(await browser.evaluate('document.body.classList.contains("dark")'),true);
  assert.equal(await browser.evaluate('document.documentElement.scrollWidth <= innerWidth'),true);
  assert.deepEqual(browser.exceptions,[]);
});

test('baseline demo navigation, exact u64 positions, state handling and clipboard',{timeout:40000},async t=>{
  const browser=await browserTest(routes);t.after(()=>browser.close());
  const chooseRows=async count=>{
    await browser.evaluate(`document.getElementById('row-count').value='${count}';document.getElementById('row-count').dispatchEvent(new Event('change'))`);
    await browser.wait(`document.getElementById('records').getAttribute('aria-rowcount')==='${BigInt(count)+1n}'`);
  };
  const selected=()=>browser.evaluate('document.querySelector("#rows tr[aria-selected=true]")?.dataset.ordinal');
  const key=async(key,code,vk,modifiers=0)=>{
    for(const type of ['keyDown','keyUp'])await browser.send('Input.dispatchKeyEvent',{type,key,code,windowsVirtualKeyCode:vk,modifiers});
  };
  await chooseRows('300000000');await browser.click('#last');
  await browser.wait('document.querySelector("#rows tr[aria-selected=true]")?.dataset.ordinal === "299999999"');
  assert.equal(await browser.evaluate('document.querySelectorAll("#rows tr").length'),12);
  assert.equal(await browser.evaluate('document.getElementById("rows").firstChild.dataset.ordinal'),'299999988');
  await chooseRows('9007199254741025');
  await browser.evaluate('document.getElementById("goto").value="9007199254741001"');await browser.click('#go');
  await browser.wait('document.querySelector("#rows tr[aria-selected=true]")?.dataset.ordinal === "9007199254741000"');
  assert.equal(await browser.evaluate('document.getElementById("goto").value'),'9007199254741001');
  await key('End','End',35);await browser.wait('document.querySelector("#rows tr[aria-selected=true]")?.dataset.ordinal === "9007199254741024"');
  await browser.evaluate('document.getElementById("goto").value="9007199254741026"');await browser.click('#go');
  assert.equal(await selected(),'9007199254741024');
  assert.match(await browser.evaluate('document.getElementById("feedback").textContent'),/Choose a row/);
  await browser.evaluate('document.getElementById("goto").value="1.5"');await browser.click('#go');
  assert.match(await browser.evaluate('document.getElementById("feedback").textContent'),/whole row/);
  await browser.click('#reset');await browser.wait('document.getElementById("records").getAttribute("aria-rowcount")==="1000001"');
  await browser.evaluate('document.getElementById("grid").focus()');await key('PageDown','PageDown',34);
  await browser.wait('document.querySelector("#rows tr[aria-selected=true]")?.dataset.ordinal === "12"');
  const before=await selected();const cursor=await browser.evaluate('document.getElementById("cursor-time").textContent');
  const point=await browser.evaluate('(()=>{const r=document.getElementById("grid").getBoundingClientRect();return{x:r.x+200,y:r.y+80}})()');
  await browser.send('Input.dispatchMouseEvent',{type:'mouseWheel',deltaY:104,deltaX:0,...point});
  await browser.wait('document.getElementById("rows").firstChild.dataset.ordinal === "5"');
  assert.equal(await selected(),before);assert.equal(await browser.evaluate('document.getElementById("cursor-time").textContent'),cursor);
  await browser.click('#waves');await browser.wait(`document.getElementById('cursor-time').textContent!==${JSON.stringify(cursor)}`);assert.equal(await selected(),before);
  await browser.click('#reveal-record');assert.match(await browser.evaluate('document.getElementById("time-window").textContent'),/ns/);
  await browser.evaluate('document.getElementById("goto").value="1234"');await browser.click('#go');
  await browser.wait('document.querySelector("#rows tr[aria-selected=true]")?.dataset.ordinal === "1233"');
  await browser.click('#copy');
  await browser.wait('document.getElementById("feedback").textContent.includes("Copied standard fields")');
  const copied=await browser.evaluate('navigator.clipboard.readText()');
  assert.equal(copied,'Generator\tID\tBegin\tEnd\tDuration\tStatus\nsoc.noc.router.request\t1234\t5932\t5944\t12\tOK');
  await browser.evaluate('navigator.clipboard.writeText=()=>Promise.reject(new DOMException("Test denial","NotAllowedError"))');
  await browser.click('#copy');await browser.wait('document.getElementById("copy-dialog").open');
  assert.equal(await browser.evaluate('document.getElementById("copy-text").value'),copied);
  assert.equal(await browser.evaluate('document.getElementById("copy-text").readOnly'),true);
  await browser.click('#copy-close');await browser.wait('!document.getElementById("copy-dialog").open');
  for(const scenario of ['loading','empty','error']){
    await browser.click(`[data-scenario="${scenario}"]`);await browser.wait(`document.getElementById('state-cover').dataset.state==='${scenario}'`);
    assert.equal(await browser.evaluate('document.querySelectorAll("#rows tr").length'),0);
    assert.equal(await browser.evaluate('document.getElementById("copy").disabled'),true);
  }
  await browser.click('#retry-load');await browser.wait('document.querySelectorAll("#rows tr").length===12');
  await browser.click('[data-scenario=loading]');await browser.wait('!!document.getElementById("cancel-load")');
  await browser.click('#cancel-load');await browser.wait('document.getElementById("source-state").textContent==="Cancelled"');
  assert.equal(await browser.evaluate('document.querySelectorAll("#rows tr").length'),0);
  await browser.click('#retry-load');await browser.wait('document.querySelectorAll("#rows tr").length===12');
  await browser.click('[data-source="response"]');await browser.wait('document.getElementById("source-path").textContent.endsWith("response")');
  assert.equal(await browser.evaluate('document.querySelector("#rows tr").children[1].textContent'),'1002');
  await browser.evaluate('document.querySelector("[data-source=request]").focus()');await key('F10','F10',121,8);
  await browser.wait('!document.getElementById("source-menu").hidden');assert.equal(await browser.evaluate('document.activeElement.id'),'open-table');await key('Enter','Enter',13);
  await browser.wait('document.getElementById("source-path").textContent.endsWith("request")');
  assert.equal(await browser.evaluate('document.getElementById("source-menu").hidden'),true);
  await browser.evaluate('document.getElementById("resident-rows").value="300000000";document.getElementById("resident-rows").dispatchEvent(new Event("input"))');
  assert.match(await browser.evaluate('document.getElementById("memory-verdict").textContent'),/^Refuse/);
  assert.deepEqual(browser.exceptions,[]);
});
