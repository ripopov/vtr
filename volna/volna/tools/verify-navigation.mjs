// Verify real browser/VS Code input against the viewer's exported state.
// Start with a loaded trace and rows; focus the editor before running.
// node verify-navigation.mjs PORT [FRAME_URL_FRAGMENT OFFSET_X OFFSET_Y]
import assert from 'node:assert/strict';
import fs from 'node:fs';

const [port, fragment, ox = '0', oy = '0'] = process.argv.slice(2);
const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
async function connect(target) {
  assert(target, 'debug target exists');
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise(r => { ws.onopen = r; });
  let id = 0;
  const pending = new Map();
  const contexts = [];
  ws.onmessage = ({data}) => {
    const m = JSON.parse(data);
    if (m.id) {
      const p = pending.get(m.id);
      if (!p) return;
      pending.delete(m.id);
      if (m.error) p.reject(new Error(JSON.stringify(m.error))); else p.resolve(m.result);
    } else if (m.method === 'Runtime.executionContextCreated') contexts.push(m.params.context);
  };
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    pending.set(++id, {resolve, reject}); ws.send(JSON.stringify({id, method, params}));
  });
  await send('Runtime.enable');
  return {ws, send, contexts};
}
const top = await connect(targets.find(t => t.type === 'page'));
const content = fragment ? await connect(targets.find(t => t.type === 'iframe' && t.url.includes(fragment))) : top;
const sleep = ms => new Promise(r => setTimeout(r, ms));
let contextId;
async function evaluate(expression, id = contextId) {
  const r = await content.send('Runtime.evaluate', {expression, contextId:id, awaitPromise:true, returnByValue:true});
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails));
  return r.result.value;
}
try {
  for (const c of content.contexts) {
    if (c.auxData?.isDefault && await evaluate("!!document.querySelector('canvas')", c.id)) { contextId = c.id; break; }
  }
  assert(contextId, 'viewer canvas context exists');
  await evaluate(`(async () => {
    const url = ${!!fragment} ? [...document.scripts].map(s=>s.textContent).join('').match(/from "([^"]+volna\\.js)"/)[1] : './dist/volna.js';
    window.navigationModule = await import(url);
    window.navigationState = () => new Promise((resolve, reject) => {
      const original = console.info;
      const timeout = setTimeout(() => { console.info = original; reject(new Error('No viewer state')); }, 3000);
      console.info = (...args) => {
        original(...args);
        const text = args.join(' ');
        if (text.includes('STATE ')) { clearTimeout(timeout); console.info = original; resolve(text); }
      };
      navigationModule.debug_state();
    });
  })()`);
  async function state() {
    const s = await evaluate('navigationState()');
    const [,a,b] = s.match(/viewport=\(([-\d.]+),([-\d.]+)\)/);
    return {start:+a, end:+b, width:+b-a, text:s};
  }
  const geometry = await evaluate('({width:innerWidth,height:innerHeight})');
  const x0 = 621, width = geometry.width - x0;
  assert(width > 150, 'wave column wide enough for gestures');
  const y = 180;
  async function mouse(type, x, y, button='left', modifiers=0) {
    await top.send('Input.dispatchMouseEvent', {type,x:x + +ox,y:y + +oy,button,modifiers,clickCount:1});
  }
  async function drag(ax,ay,bx,by,button='middle',modifiers=0,cancel=false) {
    await mouse('mouseMoved',ax,ay,'none');
    await mouse('mousePressed',ax,ay,button,modifiers);
    await mouse('mouseMoved',bx,by,button,modifiers);
    await sleep(60);
    if (cancel) await key('Escape','Escape',27);
    await mouse('mouseReleased',bx,by,button,modifiers);
    await sleep(220);
  }
  async function key(key,code,vk=0,modifiers=0) {
    await top.send('Input.dispatchKeyEvent',{type:'keyDown',key,code,windowsVirtualKeyCode:vk,modifiers});
    await top.send('Input.dispatchKeyEvent',{type:'keyUp',key,code,windowsVirtualKeyCode:vk,modifiers});
  }
  async function settledKey(...args) { await key(...args); await sleep(220); return state(); }
  const near = (a,b) => assert(Math.abs(a-b) <= Math.max(3,Math.abs(b)*1e-5), `${a} ≈ ${b}`);
  await top.send('Page.bringToFront');
  await mouse('mouseMoved',x0+width/2,y,'none');
  await mouse('mousePressed',x0+width/2,y);
  await mouse('mouseReleased',x0+width/2,y);
  const full = await settledKey('f','KeyF');
  assert(/loaded=[1-9]/.test(full.text), 'trace has loaded rows');
  for (const [button,modifiers,reverse] of [['left',2,true],['left',4,false]]) {
    await settledKey('f','KeyF');
    const a=Math.round(x0+width*.25), b=Math.round(x0+width*.75);
    await drag(reverse?b:a,y,reverse?a:b,y+80,button,modifiers);
    const selected=await state(); near(selected.width,full.width*(b-a)/width); near(selected.start,full.start+full.width*(a-x0)/width);
    await drag(a,y,b,y,button,modifiers,true);
    near((await state()).width,selected.width);
  }
  await settledKey('f','KeyF');
  await key('=','Equal'); await key('=','Equal'); await sleep(220);
  near((await state()).width,full.width/4);
  const before=await state();
  const panned=await settledKey('ArrowRight','ArrowRight',39);
  near(panned.width,before.width); near(panned.start,before.start+before.width/4);
  await mouse('mouseMoved',x0+width/2,y,'none');
  await top.send('Input.dispatchMouseEvent',{type:'mouseWheel',x:x0+width/2 + +ox,y:y + +oy,deltaX:0,deltaY:100,modifiers:0});
  await sleep(350); const wheel=await state(); near(wheel.start,panned.start+panned.width*.1);
  const zoomCursor=await settledKey('Z','KeyZ',90,8);
  near(zoomCursor.width,wheel.width/2);
  const cursor=+zoomCursor.text.match(/cursor=Some\((\d+)\)/)[1];
  near((zoomCursor.start+zoomCursor.end)/2,cursor);
  near((await settledKey('s','KeyS')).start,full.start);
  near((await settledKey('e','KeyE')).end,full.end);
  const page=await settledKey('PageDown','PageDown',34);
  near(page.end,full.end-zoomCursor.width);
  near((await settledKey('PageUp','PageUp',33)).end,full.end);
  await settledKey('f','KeyF');
  const a=Math.round(x0+width/2);
  await drag(a,y,a,y+80,'left',2); near((await state()).width,full.width);
  await settledKey('=','Equal');
  const middleBefore=await state();
  await drag(a,y,a-80,y-80,'middle');
  const middleAfter=await state(); near(middleAfter.width,middleBefore.width);
  assert(middleAfter.start>middleBefore.start,'middle drag pans later regardless of vertical drift');
  await top.send('Input.dispatchMouseEvent',{type:'mouseWheel',x:a + +ox,y:y + +oy,deltaX:0,deltaY:100,modifiers:8});
  await sleep(350); near((await state()).start,middleAfter.start);
  const rightBefore=await state();
  await drag(a,y,a-40,y,'right'); assert((await state()).start>rightBefore.start,'right drag pans later');
  const anchored=await state();
  await top.send('Input.dispatchMouseEvent',{type:'mouseWheel',x:a + +ox,y:y + +oy,deltaX:0,deltaY:-120,modifiers:2});
  await sleep(100); const zoomed=await state();
  near(zoomed.width,anchored.width/2);
  near(zoomed.start+zoomed.width*(a-x0)/width,anchored.start+anchored.width*(a-x0)/width);
  await settledKey('f','KeyF');
  const shot=await top.send('Page.captureScreenshot',{format:'png'});
  fs.writeFileSync(`/tmp/volna-navigation-${port}.png`,Buffer.from(shot.data,'base64'));
  console.log('PASS: Ctrl/Command range selection with vertical drift, Escape, repeated zoom, animated pan targets, wheel, Shift-wheel isolation, cursor zoom, endpoints, pages and middle/right-drag');
} finally {
  top.ws.close(); if (content!==top) content.ws.close();
}
