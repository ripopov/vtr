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
  const window_=()=>browser.evaluate('document.getElementById("time-window").textContent');
  const inside=await window_();
  await browser.click('#rows tr:nth-child(3)');
  assert.equal(await window_(),inside,'selecting a record already inside the window does not move the timeline');
  await browser.evaluate('document.getElementById("goto").value="4000"');await browser.click('#go');
  await browser.wait('document.querySelector("#rows tr[aria-selected=true]")?.dataset.ordinal === "3999"');
  const followed=await window_();
  assert.notEqual(followed,inside,'a selection outside the window pans the timeline to follow the cursor');
  assert.equal(await browser.evaluate(`(()=>{const [a,b]=document.getElementById('time-window').textContent.replace(' ns','').split('–').map(Number);const c=Number(document.getElementById('cursor-time').textContent.match(/\\d+/)[0]);return c>=a&&c<=b})()`),true,'the cursor is inside the followed window');
  assert.equal(await browser.evaluate('document.getElementById("cursor-time").textContent.includes("outside view")'),false);
  await browser.evaluate('document.getElementById("goto").value="1234"');await browser.click('#go');
  await browser.wait('document.querySelector("#rows tr[aria-selected=true]")?.dataset.ordinal === "1233"');
  await browser.evaluate('document.getElementById("grid").focus()');await key('c','KeyC',67,2);
  await browser.wait('document.getElementById("feedback").textContent.includes("Copied standard fields")');
  const copied=await browser.evaluate('navigator.clipboard.readText()');
  assert.equal(copied,'Generator\tID\tBegin\tEnd\tDuration\tStatus\nsoc.noc.router.request\t1234\t5932\t5944\t12\tOK');
  await browser.evaluate('navigator.clipboard.writeText=()=>Promise.reject(new DOMException("Test denial","NotAllowedError"))');
  await key('c','KeyC',67,2);await browser.wait('document.getElementById("copy-dialog").open');
  assert.equal(await browser.evaluate('document.getElementById("copy-text").value'),copied);
  assert.equal(await browser.evaluate('document.getElementById("copy-text").readOnly'),true);
  await browser.click('#copy-close');await browser.wait('!document.getElementById("copy-dialog").open');
  // Record details: one row, full attributes, events and stages.
  await browser.click('#details-toggle');await browser.wait('!document.getElementById("inspector").hidden');
  assert.equal(await browser.evaluate('document.getElementById("detail-crumb").textContent'),'soc.noc.router.request · ID 1234');
  assert.match(await browser.evaluate('document.getElementById("detail-body").textContent'),/Attributes · 26[\s\S]*Events · 12[\s\S]*Stages · 4/);
  // 20 items per list by default: attributes are cut and say so, shorter lists are not.
  assert.match(await browser.evaluate('document.getElementById("detail-body").textContent'),/vtr\.label[\s\S]*Showing 20 of 26/);
  assert.equal(await browser.evaluate('document.querySelectorAll("#detail-body .more").length'),1);
  assert.equal(await browser.evaluate('document.querySelectorAll("#detail-body table")[0].rows.length'),20);
  // The global setting changes the limit live for every list.
  await browser.click('#settings-toggle');await browser.wait('!document.getElementById("settings-menu").hidden');
  await browser.evaluate('document.getElementById("detail-items").value="3";document.getElementById("detail-items").dispatchEvent(new Event("input"))');
  await browser.wait('document.querySelectorAll("#detail-body .more").length===3');
  assert.deepEqual(await browser.evaluate('[...document.querySelectorAll("#detail-body .more")].map(p=>p.textContent.split(";")[0])'),
    ['Showing 3 of 26','Showing 3 of 12','Showing 3 of 4']);
  await browser.evaluate('document.getElementById("detail-items").value="0";document.getElementById("detail-items").dispatchEvent(new Event("input"))');
  assert.match(await browser.evaluate('document.getElementById("feedback").textContent'),/whole number of items/);
  assert.equal(await browser.evaluate('document.querySelectorAll("#detail-body .more").length'),3,'an invalid limit leaves the previous one in force');
  await browser.evaluate('document.getElementById("detail-items").value="20";document.getElementById("detail-items").dispatchEvent(new Event("input"))');
  await browser.wait('document.querySelectorAll("#detail-body .more").length===1');
  await browser.click('#source-path');await browser.wait('document.getElementById("settings-menu").hidden');
  await browser.click('#next');await browser.wait('document.getElementById("detail-crumb").textContent==="soc.noc.router.request · ID 1235"');
  await browser.click('#inspector-close');await browser.wait('document.getElementById("inspector").hidden');
  assert.equal(await browser.evaluate('document.getElementById("details-toggle").getAttribute("aria-expanded")'),'false');
  await browser.evaluate('document.getElementById("grid").focus()');await key('Enter','Enter',13);
  await browser.wait('!document.getElementById("inspector").hidden');
  await key('Escape','Escape',27);await browser.wait('document.getElementById("inspector").hidden');
  assert.notEqual(await selected(),undefined,'Escape closes details before it clears the selection');
  for(const scenario of ['loading','empty','error']){
    await browser.click(`[data-scenario="${scenario}"]`);await browser.wait(`document.getElementById('state-cover').dataset.state==='${scenario}'`);
    assert.equal(await browser.evaluate('document.querySelectorAll("#rows tr").length'),0);
    assert.equal(await browser.evaluate('document.getElementById("selected-record").textContent'),'—');
  }
  await browser.click('#retry-load');await browser.wait('document.querySelectorAll("#rows tr").length===12');
  await browser.click('[data-scenario=loading]');await browser.wait('!!document.getElementById("cancel-load")');
  await browser.click('#cancel-load');await browser.wait('document.getElementById("source-state").textContent==="Cancelled"');
  assert.equal(await browser.evaluate('document.querySelectorAll("#rows tr").length'),0);
  await browser.click('#retry-load');await browser.wait('document.querySelectorAll("#rows tr").length===12');
  await browser.click('[data-source="response"]');await browser.wait('document.getElementById("source-path").textContent.endsWith("response")');
  assert.equal(await browser.evaluate('document.querySelector("#rows tr").children[2].textContent'),'1002');
  await browser.evaluate('document.querySelector("[data-source=request]").focus()');await key('F10','F10',121,8);
  await browser.wait('!document.getElementById("source-menu").hidden');assert.equal(await browser.evaluate('document.activeElement.id'),'open-table');await key('Enter','Enter',13);
  await browser.wait('document.getElementById("source-path").textContent.endsWith("request")');
  assert.equal(await browser.evaluate('document.getElementById("source-menu").hidden'),true);
  const headers=()=>browser.evaluate('[...document.querySelectorAll("#records th")].map(th=>th.textContent).join("|")');
  assert.equal(await headers(),'Row|Label · vtr.label|Begin · ns|Duration · ns|Status|Attributes · preview');
  await browser.click('#columns-toggle');await browser.wait('!document.getElementById("columns-menu").hidden');
  assert.equal(await browser.evaluate('document.querySelector("#column-options [data-column=id]").checked'),false);
  await browser.click('#column-options [data-column=id]');
  await browser.wait('document.querySelectorAll("#records th").length===7');
  assert.equal(await headers(),'Row|Label · vtr.label|Begin · ns|Duration · ns|ID|Status|Attributes · preview');
  assert.equal(await browser.evaluate('document.querySelectorAll("#rows td").length'),84);
  assert.equal(await browser.evaluate('document.querySelector("#records").getAttribute("aria-colcount")'),'7');
  await browser.click('#column-options [data-column=label]');
  await browser.wait('document.querySelectorAll("#records th").length===6');
  assert.equal(await headers(),'Row|Begin · ns|Duration · ns|ID|Status|Attributes · preview');
  await browser.click('#columns-reset');
  await browser.wait('[...document.querySelectorAll("#records th")].map(th=>th.textContent).join("|")==="Row|Label · vtr.label|Begin · ns|Duration · ns|Status|Attributes · preview"');
  assert.equal(await browser.evaluate('document.querySelectorAll("#rows td").length'),72);
  await browser.click('#source-path');await browser.wait('document.getElementById("columns-menu").hidden');
  assert.equal(await browser.evaluate('document.getElementById("columns-toggle").getAttribute("aria-expanded")'),'false');
  await browser.evaluate('document.getElementById("resident-rows").value="300000000";document.getElementById("resident-rows").dispatchEvent(new Event("input"))');
  assert.match(await browser.evaluate('document.getElementById("memory-verdict").textContent'),/^Refuse/);
  assert.deepEqual(browser.exceptions,[]);
});

test('signal tables open from a signal selection and merge change timestamps',{timeout:40000},async t=>{
  const browser=await browserTest(routes);t.after(()=>browser.close());
  const key=async(key,code,vk,modifiers=0)=>{
    for(const type of ['keyDown','keyUp'])await browser.send('Input.dispatchKeyEvent',{type,key,code,windowsVirtualKeyCode:vk,modifiers});
  };
  const cells=n=>browser.evaluate(`[...document.querySelectorAll('#rows tr')[${n}].children].map(td=>td.textContent).join('|')`);
  await browser.click('[data-signal=addr]');
  await browser.wait('document.querySelector("[data-signal=addr]").getAttribute("aria-pressed")==="true"');
  await browser.evaluate('document.querySelector("[data-signal=clk]").focus()');await key('F10','F10',121,8);
  await browser.wait('!document.getElementById("source-menu").hidden');
  assert.equal(await browser.evaluate('document.getElementById("open-table-label").textContent'),'Open 3 signals in table');
  await key('Enter','Enter',13);
  await browser.wait('document.getElementById("records").getAttribute("aria-rowcount")==="10001"');
  assert.equal(await browser.evaluate('[...document.querySelectorAll("#records th")].map(th=>th.textContent).join("|")'),
    'Row|Time · ns|bus.clk|bus.valid|bus.addr[31:0]');
  assert.equal(await browser.evaluate('document.getElementById("source-path").textContent'),'soc.bus · 3 signals');
  assert.match(await browser.evaluate('document.getElementById("total-label").textContent'),/^10,000 rows · 80,000 B axis$/);
  // A row before a signal's first change shows no recorded value, never a zero.
  assert.equal(await cells(0),'1|1000|0|—|0x80000000');
  assert.equal(await browser.evaluate('document.querySelectorAll("#rows tr")[0].querySelectorAll("td.missing").length'),1);
  // Distinct change times only: 1002 is valid's change, 1004 clk alone,
  // 1008 clk and addr together collapsed into one row with two marked cells.
  assert.equal(await cells(1),'2|1002|0|0|0x80000000');
  assert.equal(await cells(2),'3|1004|1|0|0x80000000');
  assert.equal(await cells(3),'4|1008|0|0|0x80000040');
  assert.equal(await browser.evaluate('document.querySelectorAll("#rows tr")[3].querySelectorAll("td.changed").length'),2);
  // Selection moves the cursor to the row's timestamp; copy carries every selected signal.
  await browser.click('#rows tr:nth-child(4)');
  await browser.wait('document.getElementById("cursor-time").textContent.startsWith("Cursor 1008 ns")');
  await browser.evaluate('document.getElementById("grid").focus()');await key('c','KeyC',67,2);
  await browser.wait('document.getElementById("feedback").textContent.includes("Copied standard fields")');
  assert.equal(await browser.evaluate('navigator.clipboard.readText()'),
    'Time\tbus.clk\tbus.valid\tbus.addr[31:0]\n1008\t0\t0\t0x80000040');
  await browser.click('#details-toggle');await browser.wait('!document.getElementById("inspector").hidden');
  assert.equal(await browser.evaluate('document.getElementById("detail-crumb").textContent'),'soc.bus · 3 signals · @1008 ns');
  assert.match(await browser.evaluate('document.getElementById("detail-body").textContent'),/bus\.clk0changed here/);
  assert.match(await browser.evaluate('document.getElementById("detail-body").textContent'),/bus\.valid0held from an earlier change/);
  await browser.click('#inspector-close');await browser.wait('document.getElementById("inspector").hidden');
  // Exact navigation and the column menu work the same way over the merged axis.
  await browser.evaluate('document.getElementById("goto").value="10000"');await browser.click('#go');
  await browser.wait('document.querySelector("#rows tr[aria-selected=true]")?.dataset.ordinal === "9999"');
  await browser.click('#columns-toggle');await browser.wait('!document.getElementById("columns-menu").hidden');
  assert.equal(await browser.evaluate('[...document.querySelectorAll("#column-options input")].map(i=>i.dataset.column).join(",")'),'time,clk,valid,addr');
  await browser.click('#column-options [data-column=clk]');
  await browser.wait('document.querySelectorAll("#records th").length===4');
  // Opening a generator again restores the transaction catalogue.
  await browser.click('[data-source=request]');
  await browser.wait('document.getElementById("records").getAttribute("aria-rowcount")==="1000001"');
  assert.equal(await browser.evaluate('[...document.querySelectorAll("#records th")].map(th=>th.textContent).join("|")'),
    'Row|Label · vtr.label|Begin · ns|Duration · ns|Status|Attributes · preview');
  assert.equal(await browser.evaluate('document.getElementById("order-label").textContent'),'Source order · begin / end / ID');
  assert.deepEqual(browser.exceptions,[]);
});
