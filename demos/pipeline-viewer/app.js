'use strict';
(() => {
const $ = (id) => document.getElementById(id);
const TOTAL = 160, LABEL = 242;
const STAGES = {
  F: {name:'Fetch', color:'var(--fetch)'}, D:{name:'Decode',color:'var(--decode)'},
  R:{name:'Rename',color:'var(--rename)'}, W:{name:'Wait',color:'var(--wait)'},
  EX:{name:'Execute',color:'var(--execute)'}, C:{name:'Commit wait',color:'var(--rename)'}, RT:{name:'Retire',color:'var(--retire)'},
  SQ:{name:'Squashed',color:'var(--flush)'}
};
const esc = text => String(text).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const clamp = (x,min,max) => Math.max(min, Math.min(max,x));
const storeKey = 'vdb-pipeline-studio-v1';
let saved = {};
try { saved = JSON.parse(localStorage.getItem(storeKey) || '{}') || {}; } catch {}
const state = {scenario:'memory', selected:1012, cursor:27, zoom:24, filter:'all', search:'', deps:true, showFlushed:true, compact:false, compare:false, range:null, mode:'pan', theme:saved.theme==='dark'?'dark':'light', notes:saved.notes&&typeof saved.notes==='object'?saved.notes:{}, pins:Array.isArray(saved.pins)?saved.pins.filter(Number.isInteger):[], waveSignals:['inflight','rob','cache','lsu','valid','vdd'], bookmarks:Array.isArray(saved.bookmarks)?saved.bookmarks.filter(b=>b&&typeof b.name==='string'&&b.view).slice(0,20):[]};
let data=[], baseline=[], visible=[], toastTimer, commandIndex=0, activeCommands=[], drag=null, ignoreClick=false;
const names=['ld a0, 0(s1)','add a1, a0, s2','slli t0, a1, 2','add t1, s3, t0','ld a2, 0(t1)','xor a3, a2, a1','addi s1, s1, 8','bne s1, s4, loop'];
function generate(scenario, extra=0) {
  const ops=[];let lastRetire=-1,retireSlots=0;
  for(let i=0;i<72;i++) {
    const id=1000+i, phase=i%8;
    const mispredict=scenario!=='steady' && i===36;
    const kind=mispredict?'branch':phase===0||phase===4?'load':phase===7?'branch':'alu';
    const flushed=scenario!=='steady' && i>=37 && i<=41;
    const miss=scenario==='memory' && [12,28,56].includes(i);
    const start=Math.floor(i*1.6)+(i>=42 && scenario!=='steady'?10:0);
    let text=names[phase], producer=null, wait=0, cause='';
    if(miss) {text='ld a0, 0(s1)';wait=(i===12?18:12)+extra;cause='L1 data-cache miss';}
    if(scenario!=='steady' && i>0 && [13,14,29,30,57,58].includes(i) && scenario==='memory') {
      producer=1000+(i<20?12:i<40?28:56);
      text=i%2?'add a1, a0, s2':'slli t0, a0, 2';
      const parent=ops.find(o=>o.id===producer);
      wait=Math.max(0,parent.ready-(start+3));cause='Waiting for a0';
    } else if(phase===1 && !miss){producer=id-1;wait=Math.max(0,ops[i-1].ready-(start+3));cause=wait?'Waiting for a0':'';}
    if(mispredict){text='bne s1, s4, loop';wait=12;cause='Branch misprediction';}
    const stages=[];let t=start;
    const add=(code,length)=>{if(length>0){stages.push({code,start:t,end:t+length});t+=length;}};
    add('F',1);add('D',1);add('R',1);
    if(flushed){add('W',Math.max(1,73-t));add('SQ',1);}
    else if(miss){add('EX',1);add('W',wait);}
    else{add('W',wait || (phase===3?2:0));add('EX',kind==='load'?3:1);}
    const ready=t;
    // Small, coherent fixture rule: execute out of order, retire in order (up to two per cycle).
    if(!flushed){const retireStart=Math.max(t,lastRetire+(retireSlots===2?1:0));add('C',retireStart-t);retireSlots=retireStart===lastRetire?retireSlots+1:1;lastRetire=retireStart;add('RT',1);}
    if(!cause&&stages.some(s=>s.code==='C'))cause='Waiting to retire';
    ops.push({id,start,end:t,ready,kind:miss?'load':kind,text,pc:'0x'+(0x80001000+i*4).toString(16),stages,producer,flushed,miss,mispredict,wait:stages.filter(s=>s.code==='W'||s.code==='C').reduce((n,s)=>n+s.end-s.start,0),cause:flushed?'Branch misprediction at #1036':cause,label:miss?'load sample[i]':flushed?'wrong-path instruction':kind==='branch'?'loop back edge':'accumulate sample',sourceLine:18});
  }
  return ops;
}
function persist(){try{localStorage.setItem(storeKey,JSON.stringify({theme:state.theme,notes:state.notes,pins:state.pins,bookmarks:state.bookmarks}));}catch{notify('Storage unavailable. Your changes remain in this session.');}}
function notify(message){$('toast').textContent=message;$('toast').classList.add('show');clearTimeout(toastTimer);toastTimer=setTimeout(()=>$('toast').classList.remove('show'),2600);$('status-text').textContent=message;}
function selected(){return data.find(o=>o.id===state.selected);}
function rowHeight(){return Math.round((state.compact?24:34)*state.zoom/24*64)/64;}
function match(o){const query=state.search.toLowerCase();return (!query||`${o.id} ${o.pc} ${o.text} ${o.label} ${o.cause}`.toLowerCase().includes(query)) && (state.showFlushed||!o.flushed) && (state.filter==='all'||state.filter==='stall'&&o.wait>3||state.filter==='flush'&&o.flushed||state.filter==='load'&&o.kind==='load'||state.filter==='branch'&&o.kind==='branch');}
function stageHTML(o,s,ghost=false){const width=(s.end-s.start)*state.zoom;return ghost?`<div class="ghost-stage" style="left:${s.start*state.zoom}px;width:${Math.max(.4,width-1)}px"></div>`:`<button class="stage ${s.code==='W'?'wait':''} ${o.flushed?'flushed':''}" data-id="${o.id}" data-cycle="${s.start}" tabindex="-1" style="left:${s.start*state.zoom}px;width:${Math.max(.4,width-1)}px;background-color:${STAGES[s.code].color}" aria-label="Instruction ${o.id}, ${STAGES[s.code].name}, cycles ${s.start} to ${s.end}" title="#${o.id} · ${STAGES[s.code].name} · [${s.start}, ${s.end}) · ${s.end-s.start} cycle${s.end-s.start===1?'':'s'}${o.cause&&s.code==='W'?' · '+esc(o.cause):''}"><span class="stage-text">${width>=16&&rowHeight()>=14?s.code+(width>87&&s.end-s.start>2?' · '+(s.end-s.start)+' cyc':''):''}</span></button>`;}
function renderRows(){
  const scroll=$('chart-scroll'), oldTop=scroll.scrollTop, oldLeft=scroll.scrollLeft;
  const restoreRowFocus=!!document.activeElement?.closest('.row-label');
  const oldSelectedIndex=visible.findIndex(o=>o.id===state.selected);
  visible=data.filter(match);
  const newSelectedIndex=visible.findIndex(o=>o.id===state.selected);
  document.body.classList.toggle('compact',state.compact);
  document.documentElement.style.setProperty('--cycle-width',state.zoom+'px');
  document.body.style.setProperty('--row-height',rowHeight()+'px');
  document.body.classList.toggle('map-overview',rowHeight()<20);
  document.body.classList.toggle('map-middle',rowHeight()>=20&&rowHeight()<30);
  $('chart-content').hidden=!visible.length;$('empty').hidden=!!visible.length;
  $('rows').innerHTML=visible.map((o,i)=>`<div class="instruction-row ${o.id===state.selected?'selected':''} ${i%Math.ceil(22/rowHeight())===0?'scale-label':''}" data-row="${o.id}"><button class="row-label" data-id="${o.id}" aria-label="Select instruction ${o.id}: ${esc(o.text)}${o.flushed?', squashed':''}" aria-pressed="${o.id===state.selected}" tabindex="${o.id===state.selected?'0':'-1'}"><span class="row-id">${o.id}</span><span class="instruction-copy"><strong>${esc(o.text)}</strong><small>${o.pc}</small></span><span class="row-status ${o.flushed?'flushed':''}" title="${o.flushed?'Squashed':o.miss?'Cache miss':state.pins.includes(o.id)?'Pinned':''}">${o.flushed?'×':o.miss?'◇':state.pins.includes(o.id)?'◆':''}</span></button><div class="row-timeline">${state.compare?baseline.find(b=>b.id===o.id).stages.map(s=>stageHTML(o,s,true)).join(''):''}${o.stages.map(s=>stageHTML(o,s)).join('')}</div></div>`).join('');
  const step=state.zoom<8?16:state.zoom<16?8:state.zoom<32?4:2;
  $('ticks').innerHTML=Array.from({length:Math.floor(TOTAL/step)+1},(_,i)=>`<span style="left:${i*step*state.zoom}px">${i*step}</span>`).join('');
  $('row-count').textContent=`${visible.length} / ${data.length}`;$('search-count').textContent=state.search?`${visible.length} found`:'';
  $('zoom-label').textContent=Math.round(state.zoom/24*100)+'%';
  $('zoom-out').disabled=state.zoom<=.5;$('zoom-in').disabled=state.zoom>=64;
  $('comparison-strip').hidden=!state.compare;$('compare').setAttribute('aria-pressed',state.compare);
  scroll.scrollTop=Math.max(0,oldTop+(oldSelectedIndex>=0&&newSelectedIndex>=0?(newSelectedIndex-oldSelectedIndex)*rowHeight():0));scroll.scrollLeft=oldLeft;
  if(restoreRowFocus)document.querySelector('.row-label[aria-pressed="true"]')?.focus({preventScroll:true});
  renderArrows();renderCursor();renderRange();renderOverviewWindow();renderMetrics();
}
function renderArrows(){
  const op=selected();const svg=$('arrows');svg.setAttribute('height',visible.length*rowHeight());svg.style.height=visible.length*rowHeight()+'px';
  if(!state.deps||!op){svg.innerHTML='';return;}
  const edges=[];if(op.producer)edges.push([op.producer,op.id]);data.filter(o=>o.producer===op.id).forEach(o=>edges.push([op.id,o.id]));
  svg.innerHTML='<defs><marker id="arrow-head" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L8 4 L0 8 Z" fill="var(--accent)"/></marker></defs>'+edges.map(([a,b])=>{
    const ai=visible.findIndex(o=>o.id===a),bi=visible.findIndex(o=>o.id===b);if(ai<0||bi<0)return'';
    const source=data.find(o=>o.id===a),target=data.find(o=>o.id===b);
    const x1=source.ready*state.zoom,x2=(target.stages.find(s=>s.code==='EX')?.start||target.start)*state.zoom;
    const y1=ai*rowHeight()+rowHeight()/2,y2=bi*rowHeight()+rowHeight()/2;
    return `<path d="M${x1} ${y1} C${x1+13} ${y1},${x2+13} ${y2},${x2} ${y2}" fill="none" stroke="var(--accent)" stroke-width="${Math.min(1.6,Math.max(.35,rowHeight()/34*1.6))}" marker-end="url(#arrow-head)"/>`;
  }).join('');
}
function renderCursor(){const line=$('cursor-line');line.style.left=LABEL+state.cursor*state.zoom+'px';line.hidden=!visible.length;$('cursor-tag').textContent=state.cursor;renderWaveforms();}
function renderRange(){const overlay=$('range-overlay');overlay.hidden=!state.range;if(state.range){overlay.style.left=LABEL+state.range[0]*state.zoom+'px';overlay.style.width=(state.range[1]-state.range[0])*state.zoom+'px';$('range-start').value=state.range[0];$('range-end').value=state.range[1];}renderWaveforms();$('clear-range').disabled=!state.range;$('measure-mode').setAttribute('aria-pressed',state.mode==='measure');$('pan-mode').setAttribute('aria-pressed',state.mode==='pan');$('pan-mode').classList.toggle('selected',state.mode==='pan');$('measure-mode').classList.toggle('selected',state.mode==='measure');}
function renderMetrics(){
  const [from,to]=state.range||[0,TOTAL];const cycles=to-from;
  const retired=data.filter(o=>!o.flushed&&o.end>=from&&o.end<to).length;
  const flushed=data.filter(o=>o.flushed&&o.end>=from&&o.end<to).length;
  const waitingCycles=Array.from({length:cycles},(_,i)=>from+i).filter(c=>data.some(o=>!o.flushed&&o.stages.some(s=>['W','C'].includes(s.code)&&s.start<=c&&s.end>c))).length;
  $('analysis-bar').innerHTML=`<div class="metric"><span>${state.range?'Selected range':'Trace duration'}</span><strong>${cycles}</strong><small>cycles</small></div><div class="metric"><span>Retired</span><strong>${retired}</strong><small>ops</small></div><div class="metric"><span>Retired / cycle</span><strong>${(retired/cycles).toFixed(2)}</strong><small>IPC</small></div><div class="metric"><span>${flushed?'Squashed':'Cycles with waits'}</span><strong>${flushed||waitingCycles}</strong><small>${flushed?'ops':'cycles'}</small></div><div class="analysis-note">${state.range?`Cycles [${from}, ${to}). `:''}All instructions; filters do not change these totals.</div>`;
}
function renderOverview(){
  $('overview-svg').innerHTML=Array.from({length:41},(_,i)=>`<path d="M${i*25} 4v${i%10===0?9:4}" stroke="var(--faint)" stroke-width="1"/>`).join('');
  $('insight-strip').innerHTML=state.scenario==='memory'?`<button data-jump="1012"><i class="event-dot"></i> L1 miss <span>cyc 23</span> ↗</button><button data-jump="1036"><i class="event-dot flush"></i> Branch recovery <span>cyc 73</span> ↗</button><button data-jump="1056"><i class="event-dot"></i> Second miss cluster <span>cyc 103</span> ↗</button>`:state.scenario==='branch'?`<button data-jump="1036"><i class="event-dot flush"></i> Misprediction <span>cyc 73</span> ↗</button><button data-jump="1042"><i class="event-dot"></i> Correct path resumes <span>cyc 77</span> ↗</button>`:`<button data-jump="1012"><i class="event-dot" style="background:var(--accent)"></i> Steady execution <span>No injected misses or flushes</span> ↗</button>`;
  renderOverviewWindow();
}
function renderOverviewWindow(){const sc=$('chart-scroll');const start=sc.scrollLeft/state.zoom, span=Math.max(0,sc.clientWidth-LABEL)/state.zoom;const w=$('overview-window');w.style.left=(start/TOTAL*100)+'%';w.style.width=(Math.min(TOTAL-start,span)/TOTAL*100)+'%';$('overview').setAttribute('aria-valuenow',Math.round(start));$('overview').setAttribute('aria-valuemax',Math.max(0,Math.floor(TOTAL-span)));$('overview').setAttribute('aria-valuetext',`Cycles ${Math.floor(start)} to ${Math.min(TOTAL,Math.ceil(start+span))}`);$('visible-range').textContent=`${Math.floor(start)}–${Math.min(TOTAL,Math.ceil(start+span))} cyc`;renderWaveforms();}
// A general signal catalog: rendering depends on value type, not pipeline-specific names.
const waveCatalog=[
 {id:'inflight',name:'core.in_flight',kind:'uint',unit:'ops',color:'#328d83',max:32,value:c=>data.filter(o=>o.start<=c&&o.end>c).length},
 {id:'rob',name:'core.rob.occupancy',kind:'uint',unit:'/ 32',color:'#8778b3',max:32,value:c=>data.filter(o=>o.start+3<=c&&o.end>c).length},
 {id:'cache',name:'l1d.lines_valid',kind:'uint',unit:'/ 64',color:'#b4904a',max:64,value:c=>16+data.filter(o=>o.kind==='load'&&!o.flushed&&o.ready<=c).length},
 {id:'alu',name:'alu.execute',kind:'instruction',color:'#439b94',value:c=>data.filter(o=>o.kind!=='load'&&o.stages.some(s=>s.code==='EX'&&s.start<=c&&s.end>c)).map(o=>o.id)},
 {id:'alu-issue',name:'alu.issue',kind:'instruction',color:'#698dae',value:c=>data.filter(o=>o.kind!=='load'&&o.stages.some(s=>s.code==='EX'&&s.start===c)).map(o=>o.id)},
 {id:'lsu',name:'lsu.execute',kind:'instruction',color:'#ae925b',value:c=>data.filter(o=>o.kind==='load'&&o.stages.some(s=>s.code==='EX'&&s.start<=c&&o.ready>c)).map(o=>o.id)},
 {id:'valid',name:'core.issue_valid',kind:'bit',color:'#568bab',value:c=>Number(data.some(o=>o.stages.some(s=>s.code==='EX'&&s.start===c)))},
 {id:'flush-bit',name:'core.flush',kind:'bit',color:'#ba7189',value:c=>Number(state.scenario!=='steady'&&c===73)},
 {id:'vdd',name:'power.vdd',kind:'real',unit:'V',color:'#b07eab',min:.9,max:1.1,value:c=>1.02+.018*Math.sin(c*.3)-.04*data.filter(o=>o.stages.some(s=>s.code==='EX'&&s.start<=c&&s.end>c)).length/4}
];
function waveValue(signal,cycle){const value=signal.value(cycle);return signal.kind==='instruction'?(value.length?value.map(id=>'#'+id).join(', '):'idle'):signal.kind==='real'?value.toFixed(3)+' '+signal.unit:value+(signal.unit?' '+signal.unit:'');}
function renderWaveforms(){
 if(!$('wave-tracks')||!data.length)return;
 const sc=$('chart-scroll'),width=Math.max(1,sc.clientWidth-LABEL),start=sc.scrollLeft/state.zoom,span=width/state.zoom,end=Math.min(TOTAL,start+span);
 $('wave-cursor-value').textContent='VALUE @ '+state.cursor;
 const step=state.zoom<8?16:state.zoom<16?8:state.zoom<32?4:2;
 $('wave-ticks').innerHTML=Array.from({length:Math.ceil(end/step)+1},(_,i)=>i*step).filter(c=>c>=start&&c<=end).map(c=>`<span style="left:${(c-start)*state.zoom}px">${c}</span>`).join('');
 const signals=waveCatalog.filter(s=>state.waveSignals.includes(s.id));
 $('wave-tracks').innerHTML=signals.length?signals.map(signal=>{
   let drawing='';
   if(signal.kind==='instruction'){
     for(let c=Math.floor(start);c<end;){const ids=signal.value(c),key=ids.join(',');let next=c+1;while(next<TOTAL&&signal.value(next).join(',')===key)next++;
       if(ids.length){const left=Math.max(start,c),right=Math.min(end,next),w=(right-left)*state.zoom;
         drawing+=`<button class="wave-bus ${ids.includes(state.selected)?'related':''}" style="left:${(left-start)*state.zoom}px;width:${Math.max(1,w-1)}px;--signal-color:${signal.color}" data-wave-cycle="${c}" ${ids.length===1?`data-wave-id="${ids[0]}"`:''} title="${esc(signal.name)} · ${ids.map(id=>'#'+id).join(', ')} · [${c}, ${next})" aria-label="${esc(signal.name)}: instructions ${ids.join(', ')}, cycles ${c} to ${next}">${w>26?ids.map(id=>'#'+id).join(' + '):''}</button>`;
       }c=next;
     }
   } else if(signal.kind==='uint'){
     // Keep the scale stable across pan/zoom, using the entire example dump.
     const peak=Math.max(1,...Array.from({length:TOTAL},(_,c)=>signal.value(c)));
     let bars='';
     for(let c=Math.floor(start);c<end;c++){
       const value=signal.value(c),height=value/peak*23;
       const gap=Math.min(1.5,state.zoom*.16);
       bars+=`<rect x="${(c-start)*state.zoom+gap/2}" y="${27-height}" width="${state.zoom-gap}" height="${height}" rx="${Math.min(1,state.zoom*.08)}" fill="${signal.color}" fill-opacity="${c===state.cursor?.95:.65}"/>`;
     }
     drawing=`<svg viewBox="0 0 ${width} 30" preserveAspectRatio="none" aria-hidden="true"><path d="M0 27H${width}" stroke="var(--border)"/>${bars}</svg><span class="wave-scale" title="Fixed scale across the full example dump">0–${peak}</span>`;
   } else {
     const y=v=>signal.kind==='bit'?(v?6:23):25-(v-(signal.min||0))/((signal.max||1)-(signal.min||0))*21;
     let path='';for(let c=Math.floor(start);c<=Math.ceil(end);c++){const x=(c-start)*state.zoom,yy=y(signal.value(clamp(c,0,TOTAL-1)));path+=(c===Math.floor(start)?`M${x} ${yy}`:signal.kind==='real'?` L${x} ${yy}`:` H${x} V${yy}`);}
     drawing=`<svg viewBox="0 0 ${width} 30" preserveAspectRatio="none" aria-hidden="true"><path d="${path}" fill="none" stroke="${signal.color}" stroke-width="1.6" vector-effect="non-scaling-stroke"/></svg>`;
   }
   const cursor=(state.cursor-start)*state.zoom;
   const range=state.range?`<div class="wave-range" style="left:${(state.range[0]-start)*state.zoom}px;width:${(state.range[1]-state.range[0])*state.zoom}px"></div>`:'';
   return `<div class="wave-track" data-signal="${signal.id}"><div class="wave-label"><span class="signal-type" title="${signal.kind}" style="color:${signal.color}">${{uint:'▥',bit:'01',real:'∿',instruction:'⇥'}[signal.kind]}</span><span class="signal-name" title="${signal.name}">${signal.name}</span><output title="${esc(waveValue(signal,state.cursor))}">${esc(waveValue(signal,state.cursor))}</output></div><div class="wave-plot" data-wave-plot="${signal.id}">${range}${drawing}<i class="wave-cursor" style="left:${cursor}px" aria-hidden="true"></i></div></div>`;
 }).join(''):'<div class="wave-empty">No signals selected. Use Signals + to add a digital, numeric, or analog trace.</div>';
}
function renderSignalChooser(){const q=$('signal-search').value.toLowerCase();const found=waveCatalog.filter(s=>(s.name+' '+s.kind).includes(q));$('signal-options').innerHTML=found.length?found.map(s=>`<label class="signal-option"><input type="checkbox" data-signal-toggle="${s.id}" ${state.waveSignals.includes(s.id)?'checked':''}><span>${s.name}</span><small>${s.kind}</small></label>`).join(''):'<p class="dialog-copy">No matching signals. Try “core”, “real”, or “execute”.</p>';}
$('choose-signals').onclick=()=>{renderSignalChooser();openDialog('signals-dialog');};
$('signal-search').oninput=renderSignalChooser;
$('signal-options').onchange=e=>{const id=e.target.dataset.signalToggle;if(!id)return;state.waveSignals=e.target.checked?[...state.waveSignals,id]:state.waveSignals.filter(s=>s!==id);renderWaveforms();};
$('wave-tracks').addEventListener('click',e=>{
 const plot=e.target.closest('.wave-plot');if(!plot)return;
 const box=plot.getBoundingClientRect(),bus=e.target.closest('[data-wave-cycle]');
 const cycle=clamp(Math.floor(($('chart-scroll').scrollLeft+e.clientX-box.left)/state.zoom),0,TOTAL-1);
 if(bus?.dataset.waveId){if(!visible.some(o=>o.id===Number(bus.dataset.waveId))){state.filter='all';state.search='';state.showFlushed=true;syncControls();}selectInstruction(Number(bus.dataset.waveId),cycle);const index=visible.findIndex(o=>o.id===state.selected);if(index>=0)$('chart-scroll').scrollTop=Math.max(0,index*rowHeight()-$('chart-scroll').clientHeight/2);}
 else{state.cursor=cycle;renderCursor();}
});
$('wave-tracks').addEventListener('wheel',e=>{
 e.preventDefault();const sc=$('chart-scroll'),rect=$('wave-tracks').getBoundingClientRect();
 if(e.ctrlKey||e.metaKey)zoom(Math.exp(clamp(-e.deltaY*.003,-.7,.7)),{x:Math.max(0,e.clientX-rect.left-LABEL),y:(sc.clientHeight-33)/2});
 else sc.scrollLeft+=e.deltaX||e.deltaY;
},{passive:false});
function renderFilters(){const filters=[['all','All instructions','▤',data.length],['stall','Long waits','◷',data.filter(o=>o.wait>3).length],['load','Loads','↓',data.filter(o=>o.kind==='load').length],['branch','Branches','⑂',data.filter(o=>o.kind==='branch').length],['flush','Squashed','×',data.filter(o=>o.flushed).length]];$('filter-list').innerHTML=filters.map(([id,label,icon,count])=>`<button class="filter-button ${state.filter===id?'active':''}" data-filter="${id}" aria-pressed="${state.filter===id}"><span class="filter-symbol" aria-hidden="true">${icon}</span>${label}<span class="count">${count}</span></button>`).join('');}
function renderSaved(){
 $('bookmarks').innerHTML=state.bookmarks.length?state.bookmarks.map((b,i)=>`<div class="saved-row"><button data-bookmark="${i}" title="Restore ${esc(b.name)}">◇ ${esc(b.name)}</button><button class="delete" data-delete-bookmark="${i}" aria-label="Delete saved view ${esc(b.name)}">×</button></div>`).join(''):'<p class="empty-small">Save an interesting moment.<br>Press B or use + above.</p>';
 $('pins').innerHTML=state.pins.length?state.pins.map(id=>`<div class="saved-row"><button data-jump="${id}">◆ #${id} <span class="quiet">${esc(data.find(o=>o.id===id)?.text.split(' ')[0]||'')}</span></button><button class="delete" data-unpin="${id}" aria-label="Unpin instruction ${id}">×</button></div>`).join(''):'<p class="empty-small">Pin an instruction to keep<br>its place within reach.</p>';
}
function renderInspector(){
 const o=selected();if(!o){$('inspector').innerHTML='<h2>Choose an instruction</h2>';return;}
 const producer=data.find(p=>p.id===o.producer),consumers=data.filter(p=>p.producer===o.id),base=baseline.find(b=>b.id===o.id);
 const counts={};o.stages.forEach(s=>counts[s.code]=(counts[s.code]||0)+s.end-s.start);
 const hidden=!visible.some(v=>v.id===o.id);
 $('inspector').innerHTML=`<div class="inspector-top"><h2>INSTRUCTION</h2><div><button class="icon-button" id="previous" aria-label="Previous instruction" title="Previous instruction (↑)" ${visible.findIndex(v=>v.id===o.id)<=0?'disabled':''}>↑</button><button class="icon-button" id="next" aria-label="Next instruction" title="Next instruction (↓)" ${visible.findIndex(v=>v.id===o.id)===visible.length-1?'disabled':''}>↓</button></div></div><div class="selection-overline"><span>#${o.id} <span style="margin-left:6px">CORE 0 / T0</span></span><span class="badge ${o.flushed?'flush':''}">${o.flushed?'Squashed':'Retired'}</span></div><h3 class="instruction-title">${esc(o.text)}</h3><p class="instruction-description">${esc(o.label)} · ${o.pc}</p><div class="inspector-actions"><button class="button small" id="focus-selection" title="Focus instruction (F)">⊙ Focus <kbd>F</kbd></button><button class="button small" id="pin" aria-pressed="${state.pins.includes(o.id)}">${state.pins.includes(o.id)?'◆ Pinned':'◇ Pin'}</button></div>${hidden?'<div class="cause-card"><strong>Outside the current filter</strong><p>This selection stays available. Focus clears filters and brings it back.</p></div>':''}<dl class="detail-grid"><div><dt>Lifetime</dt><dd>${o.end-o.start} cycles</dd></div><div><dt>Wait time</dt><dd>${o.wait} cycles</dd></div><div><dt>Fetched</dt><dd>cycle ${o.start}</dd></div><div><dt>${o.flushed?'Squashed at':'Retired at'}</dt><dd>cycle ${o.end}</dd></div></dl>${o.cause?`<div class="cause-card"><strong>${o.miss?'◇ ':o.flushed?'× ':'↳ '}${esc(o.cause)}</strong><p>${o.cause==='Waiting to retire'?`Execution completed at cycle ${o.ready}. Retirement waits for older instructions and an available commit slot.`:o.mispredict?'The branch resolves at cycle 73. Five younger instructions are squashed; correct-path fetch resumes at cycle 77.':o.miss?`Memory response arrives at cycle ${o.ready}. ${consumers.length} dependent instructions wait for this result.`:o.flushed?'This instruction belongs to the wrong path. Its work does not count toward retired IPC.':`The producer #${o.producer} makes a0 available at cycle ${producer?.ready}. Follow the link below to inspect its wait.`}</p></div>`:''}${state.compare?`<div class="compare-delta"><strong>${base.end===o.end?'No change':`${base.end-o.end} cycles earlier`}</strong><span>Candidate retirement ${o.end} · baseline ${base.end}<br>Matched on instruction #${o.id}</span></div>`:''}<section class="inspector-section"><h3>Stage breakdown <span>${o.end-o.start} cycles total</span></h3>${Object.entries(counts).map(([code,n])=>`<div class="duration-row"><i class="swatch" style="background:${STAGES[code].color}"></i><span class="stage-name">${STAGES[code].name}</span><span class="duration-track"><i style="width:${n/(o.end-o.start)*100}%;background:${STAGES[code].color}"></i></span><span class="duration-value">${n} cyc</span></div>`).join('')}</section><section class="inspector-section"><h3>Dependencies <span>${(producer?1:0)+consumers.length} direct links</span></h3>${producer?relation(producer,'Producer · a0','↖'):''}${consumers.map(c=>relation(c,'Consumer · a0','↳')).join('')}${!producer&&!consumers.length?'<p class="empty-small">No register dependencies in this example.</p>':''}</section><section class="inspector-section"><h3>Source context <span>example.c:${o.sourceLine}</span></h3><div class="source-snippet">17  for (i = 0; i &lt; n; i++) {\n<b>18    sum += sample[i];</b>\n19  }</div><p class="empty-small" style="margin-top:6px">Illustrative source mapping</p></section><section class="inspector-section"><h3>Investigation note</h3><label class="note-label" for="instruction-note">Private note for #${o.id} in this trace</label><textarea id="instruction-note" placeholder="What did you learn?" maxlength="1000">${esc(state.notes[noteKey(o.id)]||'')}</textarea><div class="note-footer"><span>Saved in this browser</span><button id="save-note" class="button small">Save note</button></div></section>`;
 $('previous').onclick=()=>stepInstruction(-1);$('next').onclick=()=>stepInstruction(1);$('focus-selection').onclick=()=>focusSelection();$('pin').onclick=()=>togglePin(o.id);$('save-note').onclick=()=>{state.notes[noteKey(o.id)]=$('instruction-note').value;persist();notify(`Note saved for #${o.id}`);};
}
function noteKey(id){return state.scenario+':'+id;}
function relation(o,label,icon){return `<button class="relation-button" data-jump="${o.id}"><span aria-hidden="true">${icon}</span><div><strong>#${o.id} ${esc(o.text)}</strong><small>${label}</small></div><em aria-hidden="true">→</em></button>`;}
function selectInstruction(id,cycle,focus=false){if(!data.some(o=>o.id===id))return;const changed=state.selected!==id;state.selected=id;if(Number.isFinite(cycle))state.cursor=clamp(cycle,0,TOTAL-1);renderRows();renderInspector();if(changed)$('inspector').scrollTop=0;if(focus)focusSelection();$('status-text').textContent=`#${id} selected · ${selected().text}`;}
function focusSelection(){const o=selected();if(!o)return;if(!match(o)){clearFilters(false);}const sc=$('chart-scroll'),available=Math.max(120,sc.clientWidth-LABEL);const end=state.compare?Math.max(o.end,baseline.find(b=>b.id===o.id).end):o.end;state.zoom=Math.max(24,state.zoom);if((end-o.start+4)*state.zoom>available)state.zoom=clamp(available/(end-o.start+4),.5,64);renderRows();sc.scrollLeft=Math.max(0,(o.start-2)*state.zoom);const i=visible.findIndex(v=>v.id===o.id);sc.scrollTop=Math.max(0,i*rowHeight()-sc.clientHeight/3);renderOverviewWindow();renderInspector();}
function stepInstruction(dir){if(!visible.length)return;let i=visible.findIndex(o=>o.id===state.selected);i=clamp(i+dir,0,visible.length-1);selectInstruction(visible[i].id);const sc=$('chart-scroll'),top=i*rowHeight();if(top<sc.scrollTop||top>sc.scrollTop+sc.clientHeight-rowHeight()-33)sc.scrollTop=Math.max(0,top-sc.clientHeight/2);}
// Anchor coordinates are in the scroll viewport, excluding the fixed label column and ruler.
function mapAnchor(point){
  const sc=$('chart-scroll');
  const x=point?.x??Math.max(0,sc.clientWidth-LABEL)/2;
  const y=point?.y??Math.max(0,sc.clientHeight-33)/2;
  return {cycle:(sc.scrollLeft+x)/state.zoom,row:(sc.scrollTop+y)/rowHeight(),x,y};
}
function zoomTo(value,anchor){
  const sc=$('chart-scroll');state.zoom=clamp(value,.5,64);renderRows();
  sc.scrollLeft=anchor.cycle*state.zoom-anchor.x;
  sc.scrollTop=anchor.row*rowHeight()-anchor.y;
  renderOverviewWindow();
}
function zoom(factor,point){zoomTo(state.zoom*factor,mapAnchor(point));}
function fit(){
  const sc=$('chart-scroll');
  state.zoom=clamp(Math.min((sc.clientWidth-LABEL)/TOTAL,(sc.clientHeight-33)*24/(Math.max(1,visible.length)*(state.compact?24:34))),.5,64);
  renderRows();sc.scrollLeft=0;sc.scrollTop=0;renderOverviewWindow();notify('Entire pipeline fitted · cycles and instructions');
}
function clearFilters(render=true){state.search='';state.filter='all';state.showFlushed=true;$('search').value='';$('show-flushed').checked=true;renderFilters();if(render){renderRows();renderInspector();}}
function filterBy(value){state.filter=value;if(value==='flush'){state.showFlushed=true;$('show-flushed').checked=true;}renderFilters();renderRows();renderInspector();}
function toggleCompare(){state.compare=!state.compare;renderRows();renderInspector();if(state.compare)focusSelection();notify(state.compare?'Baseline overlaid · aligned by instruction ID':'Comparison hidden');}
function togglePin(id){state.pins=state.pins.includes(id)?state.pins.filter(p=>p!==id):[...state.pins,id];persist();renderSaved();renderRows();renderInspector();}
function loadScenario(name){state.scenario=['memory','branch','steady'].includes(name)?name:'memory';$('scenario').value=state.scenario;data=generate(state.scenario);baseline=generate(state.scenario,8);renderFilters();renderRows();renderOverview();renderInspector();renderSaved();$('inspector').scrollTop=0;}
function theme(){document.body.classList.toggle('dark',state.theme==='dark');$('theme').textContent=state.theme==='dark'?'☀':'◐';$('theme').setAttribute('aria-label',state.theme==='dark'?'Switch to light theme':'Switch to dark theme');}
function toggleTheme(){state.theme=state.theme==='light'?'dark':'light';theme();persist();}
function reset(){Object.assign(state,{scenario:'memory',selected:1012,cursor:27,zoom:24,filter:'all',search:'',deps:true,showFlushed:true,compact:false,compare:false,range:null,mode:'pan'});syncControls();loadScenario('memory');focusSelection();}
function syncControls(){$('search').value=state.search;$('dependencies').checked=state.deps;$('show-flushed').checked=state.showFlushed;$('compact').checked=state.compact;$('scenario').value=state.scenario;}
function captureView(){const sc=$('chart-scroll');return {scenario:state.scenario,selected:state.selected,cursor:state.cursor,zoom:state.zoom,filter:state.filter,search:state.search,deps:state.deps,showFlushed:state.showFlushed,compact:state.compact,compare:state.compare,range:state.range,waveSignals:[...state.waveSignals],left:sc.scrollLeft,top:sc.scrollTop};}
function restore(view){const v=view||{};if(Array.isArray(v.waveSignals))state.waveSignals=v.waveSignals.filter(id=>waveCatalog.some(s=>s.id===id));Object.assign(state,{scenario:['memory','branch','steady'].includes(v.scenario)?v.scenario:'memory',selected:clamp(Number(v.selected)||1012,1000,1071),cursor:clamp(Number(v.cursor)||0,0,159),zoom:clamp(Number(v.zoom)||24,.5,64),filter:['all','load','branch','stall','flush'].includes(v.filter)?v.filter:'all',search:typeof v.search==='string'?v.search:'',deps:v.deps!==false,showFlushed:v.showFlushed!==false,compact:!!v.compact,compare:!!v.compare,range:Array.isArray(v.range)&&v.range.length===2&&v.range[0]>=0&&v.range[1]<=TOTAL&&v.range[0]<v.range[1]?v.range:null});syncControls();loadScenario(state.scenario);$('chart-scroll').scrollLeft=Math.max(0,Number(v.left)||0);$('chart-scroll').scrollTop=Math.max(0,Number(v.top)||0);renderOverviewWindow();}
function bookmark(){openDialog('save-dialog');$('bookmark-name').value=`${selected()?.cause||'Instruction #'+state.selected}`;$('bookmark-name').focus();$('bookmark-name').select();}
function applyRange(){const a=Number($('range-start').value),b=Number($('range-end').value);if(!Number.isInteger(a)||!Number.isInteger(b)||a<0||b>TOTAL||a>=b){notify('Use whole cycles from 0 to 160, with the end after the start.');return;}state.range=[a,b];renderRange();renderMetrics();notify(`Measured ${b-a} cycles · [${a}, ${b})`);}
function openDialog(id){$(id).showModal();}
function showTab(name,focus=false){const tutorial=name==='tutorial';$('viewer').hidden=tutorial;$('tutorial').hidden=!tutorial;for(const item of ['viewer','tutorial']){const active=item===name;$('tab-'+item).setAttribute('aria-selected',active);$('tab-'+item).tabIndex=active?0:-1;}if(focus)$('tab-'+name).focus();if(!tutorial)requestAnimationFrame(()=>{renderOverviewWindow();});}
const shortcuts=[['Search instructions','/'],['Command palette','⌘ / Ctrl','K'],['Previous / next instruction','↑','↓'],['Move cycle cursor','←','→'],['Map zoom in / out','+','−'],['Zoom at pointer','Ctrl / ⌘','wheel'],['Zoom at point','Double-click'],['Zoom out at point','Shift','Double-click'],['Touch map zoom','Pinch'],['Focus selected instruction','F'],['Fit full trace','0'],['Measure mode','M'],['Toggle dependencies','D'],['Compare baseline','C'],['Bookmark view','B'],['Pin selected instruction','P'],['Clear search / range','Esc'],['Show this reference','?']];
$('shortcut-list').innerHTML=shortcuts.map(([label,...keys])=>`<div class="shortcut"><span>${label}</span><span>${keys.map(k=>`<kbd>${k}</kbd>`).join('')}</span></div>`).join('');
function commandDefinitions(){return [
 ['Focus selected instruction','F',focusSelection],['Fit entire trace','0',fit],['Compare with baseline','C',toggleCompare],['Measure a range','M',()=>{state.mode='measure';renderRange();$('range-start').focus();}],['Save view as bookmark','B',bookmark],['Pin selected instruction','P',()=>togglePin(state.selected)],['Toggle dependency arrows','D',()=>{$('dependencies').click();}],['Show long waits','',()=>filterBy('stall')],['Show all instructions','',()=>clearFilters()],['Open branch recovery example','',()=>{clearFilters(false);loadScenario('branch');selectInstruction(1036,73,true);}],['Switch appearance','',toggleTheme],['Keyboard shortcuts','?',()=>openDialog('shortcuts')],['Open the field guide','',()=>showTab('tutorial')],['Reset view','',reset]
 ];}
function renderCommands(){const q=$('command-search').value.trim().toLowerCase();activeCommands=commandDefinitions().filter(c=>c[0].toLowerCase().includes(q));if(/^#?\d+$/.test(q)){const n=Number(q.replace('#',''));if(n>=1000&&n<=1071)activeCommands.unshift([`Go to instruction #${n}`,'',()=>selectInstruction(n,undefined,true)]);if(!q.startsWith('#')&&n>=0&&n<TOTAL)activeCommands.unshift([`Go to cycle ${n}`,'',()=>{state.cursor=n;$('chart-scroll').scrollLeft=Math.max(0,(n-6)*state.zoom);renderCursor();renderOverviewWindow();}]);}commandIndex=clamp(commandIndex,0,Math.max(0,activeCommands.length-1));$('command-list').innerHTML=activeCommands.length?activeCommands.map(([label,key],i)=>`<button class="command-item" id="command-${i}" role="option" aria-selected="${i===commandIndex}" data-command="${i}" tabindex="-1"><span>${esc(label)}</span>${key?`<kbd>${key}</kbd>`:''}</button>`).join(''):'<p class="dialog-copy">No matching command. Try “focus”, “compare”, a cycle, or #1012.</p>';$('command-search').setAttribute('aria-activedescendant',activeCommands.length?'command-'+commandIndex:'');}
function openCommands(){commandIndex=0;$('command-search').value='';renderCommands();openDialog('palette');$('command-search').focus();}
function runCommand(i){const c=activeCommands[i];if(!c)return;$('palette').close();c[2]();}
function exportView(){const report={title:'VDB Pipeline Studio investigation',trace:state.scenario,synthetic:true,view:captureView(),instruction:selected(),metricsScope:'all instructions, regardless of display filters',note:state.notes[noteKey(state.selected)]||''};const blob=new Blob([JSON.stringify(report,null,2)],{type:'application/json'});const link=document.createElement('a');link.href=URL.createObjectURL(blob);link.download=`pipeline-${state.scenario}-${state.selected}.json`;link.click();setTimeout(()=>URL.revokeObjectURL(link.href),1000);notify('Investigation exported as JSON');}
$('search').oninput=()=>{state.search=$('search').value;renderRows();renderInspector();};
$('search').onkeydown=e=>{if(e.key==='Enter'&&visible.length){selectInstruction(visible[0].id,undefined,true);$('chart-scroll').focus();}if(e.key==='Escape'){e.preventDefault();e.stopPropagation();clearFilters();$('chart-scroll').focus();}};
$('scenario').onchange=()=>{clearFilters(false);state.range=null;loadScenario($('scenario').value);selectInstruction(state.scenario==='branch'?1036:1012,undefined,true);};
$('dependencies').onchange=()=>{state.deps=$('dependencies').checked;renderArrows();};$('show-flushed').onchange=()=>{state.showFlushed=$('show-flushed').checked;renderRows();renderInspector();};$('compact').onchange=()=>{state.compact=$('compact').checked;renderRows();};
$('zoom-in').onclick=()=>zoom(1.25);$('zoom-out').onclick=()=>zoom(.8);$('fit').onclick=fit;$('compare').onclick=toggleCompare;$('commands').onclick=openCommands;$('help').onclick=()=>openDialog('shortcuts');$('theme').onclick=toggleTheme;$('reset').onclick=()=>{reset();notify('Initial view restored');};$('export').onclick=exportView;$('bookmark').onclick=bookmark;$('clear-filters').onclick=()=>clearFilters();$('apply-range').onclick=applyRange;$('clear-range').onclick=()=>{state.range=null;renderRange();renderMetrics();};
$('pan-mode').onclick=()=>{state.mode='pan';renderRange();};$('measure-mode').onclick=()=>{state.mode='measure';renderRange();notify('Drag across the timeline to measure, or enter a range below.');};
$('quick-guide').onclick=()=>showTab('tutorial');$('back-viewer').onclick=()=>showTab('viewer');$('tab-viewer').onclick=()=>showTab('viewer');$('tab-tutorial').onclick=()=>showTab('tutorial');
for(const id of ['tab-viewer','tab-tutorial'])$(id).onkeydown=e=>{if(['ArrowLeft','ArrowRight','Home','End'].includes(e.key)){e.preventDefault();showTab(e.key==='Home'?'viewer':e.key==='End'?'tutorial':id==='tab-viewer'?'tutorial':'viewer',true);}};
$('save-form').onsubmit=e=>{e.preventDefault();const name=$('bookmark-name').value.trim();if(!name){$('bookmark-name').setCustomValidity('Give this view a name.');$('bookmark-name').reportValidity();return;}if(state.bookmarks.length>=20){notify('You have 20 saved views. Delete one to make room.');return;}state.bookmarks.push({name,view:captureView()});persist();renderSaved();$('save-dialog').close();notify(`Saved “${name}”`);};$('bookmark-name').oninput=()=>$('bookmark-name').setCustomValidity('');
$('command-search').setAttribute('role','combobox');$('command-search').setAttribute('aria-controls','command-list');$('command-search').setAttribute('aria-expanded','true');$('command-search').oninput=()=>{commandIndex=0;renderCommands();};$('command-search').onkeydown=e=>{if(['ArrowDown','ArrowUp'].includes(e.key)){e.preventDefault();commandIndex=clamp(commandIndex+(e.key==='ArrowDown'?1:-1),0,activeCommands.length-1);renderCommands();$('command-'+commandIndex)?.scrollIntoView({block:'nearest'});}if(e.key==='Enter'){e.preventDefault();runCommand(commandIndex);}};
document.addEventListener('click',e=>{
 const b=e.target.closest('button');if(!b)return;
 if(b.dataset.close)$(b.dataset.close).close();
 if(b.dataset.filter)filterBy(b.dataset.filter);
 if(b.dataset.id&&!ignoreClick)selectInstruction(Number(b.dataset.id),b.dataset.cycle===undefined?undefined:Number(b.dataset.cycle));
 if(b.dataset.jump){selectInstruction(Number(b.dataset.jump),undefined,true);}
 if(b.dataset.bookmark!==undefined){restore(state.bookmarks[Number(b.dataset.bookmark)].view);notify('Saved view restored');}
 if(b.dataset.deleteBookmark!==undefined){state.bookmarks.splice(Number(b.dataset.deleteBookmark),1);persist();renderSaved();notify('Saved view deleted');}
 if(b.dataset.unpin){state.pins=state.pins.filter(id=>id!==Number(b.dataset.unpin));persist();renderSaved();renderRows();renderInspector();}
 if(b.dataset.command!==undefined)runCommand(Number(b.dataset.command));
 if(b.dataset.lesson)lesson(b.dataset.lesson);
 if(b.dataset.image){$('expanded-image').src=b.dataset.image;$('expanded-image').alt=b.querySelector('img').alt;openDialog('image-dialog');}
});
const sc=$('chart-scroll');sc.addEventListener('scroll',renderOverviewWindow,{passive:true});
function pointerPoint(e){const box=sc.getBoundingClientRect();return {x:Math.max(0,e.clientX-box.left-LABEL),y:Math.max(0,e.clientY-box.top-33)};}
sc.addEventListener('wheel',e=>{
  if(e.ctrlKey||e.metaKey){
    e.preventDefault();
    const pixels=e.deltaY*(e.deltaMode===1?16:e.deltaMode===2?sc.clientHeight:1);
    zoom(Math.exp(clamp(-pixels*.003,-.7,.7)),pointerPoint(e));
  }
},{passive:false});
let lastTimelineTap=null,lastTimelineDouble=0;
sc.addEventListener('dblclick',e=>{
  if(performance.now()-lastTimelineDouble<150)return;
  if(e.target.closest('.row-label')||e.clientX-sc.getBoundingClientRect().left<LABEL)return;
  e.preventDefault();zoom(e.shiftKey?.5:2,pointerPoint(e));
});
function timelineCycle(e){const r=sc.getBoundingClientRect();return clamp(Math.floor((e.clientX-r.left+sc.scrollLeft-LABEL)/state.zoom),0,TOTAL-1);}
const touches=new Map();let pinch=null;
function touchPair(){const [a,b]=[...touches.values()];return {distance:Math.max(1,Math.hypot(b.x-a.x,b.y-a.y)),x:(a.x+b.x)/2,y:(a.y+b.y)/2};}
sc.addEventListener('pointerdown',e=>{
  if(e.button!==0||e.target.closest('.row-label')||e.target.closest('.empty-state'))return;
  const bounds=sc.getBoundingClientRect();if(e.clientX-bounds.left<LABEL)return;
  sc.setPointerCapture(e.pointerId);
  if(e.pointerType==='touch'){
    touches.set(e.pointerId,pointerPoint(e));
    if(touches.size>=2){const pair=touchPair();pinch={...pair,zoom:state.zoom,anchor:mapAnchor(pair)};drag=null;return;}
  }
  drag={x:e.clientX,y:e.clientY,left:sc.scrollLeft,top:sc.scrollTop,cycle:timelineCycle(e),measure:state.mode==='measure'||e.shiftKey,moved:false,targetId:Number(e.target.closest('.stage')?.dataset.id),stageCycle:Number(e.target.closest('.stage')?.dataset.cycle)};
});
sc.addEventListener('pointermove',e=>{
  if(touches.has(e.pointerId))touches.set(e.pointerId,pointerPoint(e));
  if(pinch){if(touches.size>=2){const pair=touchPair();zoomTo(pinch.zoom*pair.distance/pinch.distance,{...pinch.anchor,x:pair.x,y:pair.y});}return;}
  if(!drag)return;const dx=e.clientX-drag.x,dy=e.clientY-drag.y;
  if(Math.abs(dx)+Math.abs(dy)>4)drag.moved=true;if(!drag.moved)return;
  if(drag.measure){const c=timelineCycle(e);state.range=[Math.min(c,drag.cycle),Math.max(c,drag.cycle)+1];renderRange();renderMetrics();}
  else{sc.scrollLeft=drag.left-dx;sc.scrollTop=drag.top-dy;}
});
sc.addEventListener('pointerup',e=>{
  touches.delete(e.pointerId);
  if(pinch){if(!touches.size)pinch=null;drag=null;ignoreClick=true;setTimeout(()=>ignoreClick=false,0);return;}
  if(!drag)return;
  if(!drag.moved&&e.pointerType==='mouse'){
    const now=performance.now();
    if(lastTimelineTap&&now-lastTimelineTap.time<400&&Math.hypot(e.clientX-lastTimelineTap.x,e.clientY-lastTimelineTap.y)<4){lastTimelineTap=null;lastTimelineDouble=now;zoom(e.shiftKey?.5:2,pointerPoint(e));drag=null;ignoreClick=true;setTimeout(()=>ignoreClick=false,0);return;}
    lastTimelineTap={time:now,x:e.clientX,y:e.clientY};
  }else lastTimelineTap=null;
  if(!drag.moved){if(drag.targetId){selectInstruction(drag.targetId,drag.stageCycle);sc.focus({preventScroll:true});}else{state.cursor=timelineCycle(e);renderCursor();}}
  ignoreClick=drag.moved||!!drag.targetId;drag=null;setTimeout(()=>ignoreClick=false,0);
});
sc.addEventListener('pointercancel',e=>{touches.delete(e.pointerId);if(!touches.size)pinch=null;drag=null;});
$('overview').addEventListener('pointerdown',e=>{const box=$('overview').getBoundingClientRect();const span=(sc.clientWidth-LABEL)/state.zoom;const move=event=>{const c=(event.clientX-box.left)/box.width*TOTAL;sc.scrollLeft=Math.max(0,(c-span/2)*state.zoom);};move(e);$('overview').setPointerCapture(e.pointerId);const onMove=event=>move(event),end=()=>{$('overview').removeEventListener('pointermove',onMove);$('overview').removeEventListener('pointerup',end);};$('overview').addEventListener('pointermove',onMove);$('overview').addEventListener('pointerup',end);});
$('overview').onkeydown=e=>{if(['ArrowLeft','ArrowRight','Home','End'].includes(e.key)){e.preventDefault();sc.scrollLeft=e.key==='Home'?0:e.key==='End'?sc.scrollWidth:sc.scrollLeft+(e.key==='ArrowRight'?8:-8)*state.zoom;}};
document.addEventListener('keydown',e=>{
 const editing=e.target.matches('input,textarea,select,[contenteditable]');if((e.metaKey||e.ctrlKey)&&e.key.toLowerCase()==='k'){e.preventDefault();if(!$('palette').open)openCommands();return;}
 if(document.querySelector('dialog[open]')||editing||!$('tutorial').hidden||e.target.closest('[role=tablist]'))return;
 if(e.metaKey||e.ctrlKey||e.altKey)return;
 switch(e.key){case '/':e.preventDefault();$('search').focus();break;case '?':e.preventDefault();openDialog('shortcuts');break;case 'ArrowUp':case 'ArrowDown':if(e.target===sc||e.target.closest('.row-label')){e.preventDefault();stepInstruction(e.key==='ArrowUp'?-1:1);}break;case 'ArrowLeft':case 'ArrowRight':if(e.target===sc||e.target.closest('.row-label')){e.preventDefault();state.cursor=clamp(state.cursor+(e.key==='ArrowLeft'?-1:1),0,TOTAL-1);renderCursor();const x=state.cursor*state.zoom;if(x<sc.scrollLeft||x>sc.scrollLeft+sc.clientWidth-LABEL)sc.scrollLeft=Math.max(0,x-(sc.clientWidth-LABEL)/2);}break;case '+':case '=':e.preventDefault();zoom(1.25);break;case '-':e.preventDefault();zoom(.8);break;case '0':e.preventDefault();fit();break;case 'f':case 'F':focusSelection();break;case 'c':case 'C':toggleCompare();break;case 'b':case 'B':bookmark();break;case 'p':case 'P':togglePin(state.selected);break;case 'd':case 'D':$('dependencies').click();break;case 'm':case 'M':state.mode=state.mode==='measure'?'pan':'measure';renderRange();break;case 'Escape':clearFilters();state.range=null;state.mode='pan';renderRange();renderMetrics();break;}
});
new ResizeObserver(()=>renderOverviewWindow()).observe(sc);
const lessons=[
 {id:'overview',title:'Read waveforms beside execution',intro:'The top pane is a general-purpose waveform viewer. The bottom pane shows instruction lifetimes. Both use the same cycle axis, zoom, cursor, and measured interval.',steps:['Use <strong>Signals +</strong> to mix in-flight instructions, ROB and cache occupancy, execution-stage instruction IDs, digital control signals, and analog supply voltage. Search the chooser by name or type.','Each signal shows its <strong>value at the shared cursor</strong>. Click any waveform to move that cursor; selecting a pipeline stage updates all waveform values. Click a labeled execution span to select its instruction below.','Pan or zoom the pipeline and the waveforms follow exactly. Scroll horizontally over the waveforms to pan both panes, or use Ctrl/Command + wheel to zoom. The thin <strong>Time navigator</strong> locates the shared window within the full dump.'],caption:'A general waveform pane above the pipeline: occupancy bars, instruction-valued execution, a digital control signal, and analog voltage share the same time axis.',tip:'All signal values are illustrative. The demo does not import a real waveform dump.'},
 {id:'navigate',title:'Move without losing your place',intro:'The waveform pane and pipeline share time while the navigator preserves the full-dump context. Instruction labels and the cycle ruler stay in place as you move.',steps:['Choose <strong>Pan</strong> and drag the timeline. Native horizontal scrolling and trackpad gestures work too. Scroll vertically to move through instruction rows.','Zoom like a map: <strong>Ctrl/Command + wheel</strong> or a <strong>two-finger pinch</strong> scales both cycles and instruction rows around the gesture. <strong>Double-click</strong> zooms in; <strong>Shift + double-click</strong> zooms out. The + / − controls zoom around the center. Labels simplify at small scales. C is a commit wait: execution is done, but retirement is waiting for older work.','Use <strong>Fit</strong> (0) to see all cycles and instruction rows, <strong>Focus</strong> (F) to frame the selected instruction, and <strong>Reset view</strong> to restore the opening example.'],caption:'Map zoom fits the entire pipeline in both dimensions, exposing the shape of cache stalls and recovery.',tip:'Focus clears filters only when needed to reveal the selected instruction.'},
 {id:'inspect',title:'Make one instruction the anchor',intro:'Click a label or stage to select it. The inspector remains beside the trace, so the evidence and explanation stay visible together.',steps:['Select <strong>#1012</strong>. Its inspector shows the disassembly, PC, lifetime, fetch and retire cycles, and exact time spent in each stage.','Hover a stage for its <strong>cycle interval</strong>. Selecting a stage also places the blue cycle cursor at its start. Wait spans carry labels as well as color.','Focus the timeline and use <strong>↑ / ↓</strong> to select neighboring instructions and <strong>← / →</strong> to move the cycle cursor. The inspector’s arrow buttons offer the same instruction navigation.'],caption:'A selected load, its 18-cycle memory wait, and the per-stage breakdown.',tip:'Intervals are half-open: [23, 41) represents 18 cycles.'},
 {id:'dependencies',title:'Follow the wait to its cause',intro:'A waiting instruction is a symptom. Follow its producer to find the cause, then return through the consumer links.',steps:['Select <strong>#1013</strong>, which waits for register a0. The cause card identifies its producer and the cycle when that value becomes available.','Click the <strong>Producer #1012</strong> link. The timeline frames the load, and direct consumers appear in the inspector. Follow either consumer to continue the investigation.','Toggle <strong>Dependency arrows</strong> (D) to show or hide the selected instruction’s direct links. Unrelated arrows stay hidden to keep the graph readable.'],caption:'The dependent add is selected. Its producer link leads directly to the load responsible for the wait.',tip:'Links still work when a producer is off-screen or excluded by a filter.'},
 {id:'search',title:'Find the signal in the noise',intro:'Search and investigation filters narrow the rows immediately. Selection stays available in the inspector even if it falls outside the current results.',steps:['Press <strong>/</strong> and enter an opcode, instruction ID, PC, or label. Search is case-insensitive literal text; try <strong>ld</strong> or <strong>1012</strong>.','Combine search with <strong>Loads</strong>, <strong>Long waits</strong>, <strong>Branches</strong>, or <strong>Squashed</strong>. The result count always shows how many rows remain. Enter focuses the first match.','Use the search field’s clear control or <strong>Escape</strong> to reset filters. If nothing matches, the empty state provides a single <strong>Clear filters</strong> action.'],caption:'Load instructions filtered by “ld”, with the original selection and inspector retained.',tip:'Display filters never change the trace-wide or range metrics.'},
 {id:'flush',title:'Distinguish wasted work',intro:'Squashed instructions remain visible by default. A cross, hatching, and a text status distinguish them from retired work.',steps:['Open <strong>Branch recovery</strong> from the trace selector or click the <strong>Branch recovery</strong> event chip.','Choose <strong>Squashed</strong> to isolate wrong-path instructions. The inspector explains why they were discarded.','Toggle <strong>Show flushed</strong> off when examining useful execution. Squashed work is always excluded from retired IPC, including when it is visible.'],caption:'Wrong-path operations are hatched and labeled Squashed, preserving the difference between activity and useful work.',tip:'These examples illustrate trace reading; they are not measurements from a processor.'},
 {id:'measure',title:'Turn a pattern into a measurement',intro:'Measure time explicitly rather than estimating from pixels. The range overlay and summary use the same cycle boundaries.',steps:['Choose <strong>Measure</strong> (M), then drag across the timeline. <strong>Shift + drag</strong> measures temporarily while Pan is selected.','For an exact result, enter <strong>23</strong> and <strong>41</strong> below the timeline and choose <strong>Apply</strong>. This isolates the load’s 18-cycle memory wait.','The summary reports range duration, instructions retiring in that interval, retired IPC, and squashed operations or cycles containing waits. Choose <strong>×</strong> to return to full-trace totals.'],caption:'The [23, 41) interval is measured; summary values are computed from all instructions in the range.',tip:'An instruction contributes to retired IPC only when its retirement falls within the measured interval.'},
 {id:'compare',title:'Compare at the same instruction',intro:'A baseline is useful only when you know how it is aligned. This comparison uses stable instruction IDs, with both runs sharing one cycle axis.',steps:['Choose <strong>Compare</strong> (C). Dashed outlines show the baseline; filled stages remain the candidate. Both pan and zoom together.','Inspect <strong>#1012</strong>. In this example the baseline memory wait is eight cycles longer, which also delays its dependent instructions.','Read the inspector’s <strong>retirement delta</strong>, then navigate to a consumer. Toggle Compare again to return to one trace. No synthetic speedup percentage is presented as a real benchmark.'],caption:'A dashed baseline extends beyond the candidate load, with an eight-cycle retirement difference in the inspector.',tip:'The baseline is a deterministic variant of the same example, not an imported run.'},
 {id:'bookmarks',title:'Keep useful places within reach',intro:'Bookmarks save the investigation context. Pins keep specific instruction identities available as you move through the trace.',steps:['Use <strong>Pin</strong> (P) in the inspector to add an instruction to the sidebar. Click a pinned entry to return to it; use its × control to unpin.','Press <strong>B</strong> or the + beside Saved views. Give the view a meaningful name such as <strong>First memory stall</strong>.','Restore a saved view to recover its trace, selection, filters, time position, zoom, comparison, and measurement. Use the × beside a saved view to delete it.'],caption:'A named bookmark is ready to save; the selected instruction is pinned in the sidebar.',tip:'Bookmarks, pins, notes, and appearance persist in this browser when local storage is available.'},
 {id:'notes',title:'Leave an explanation, not just a location',intro:'A useful investigation records what you learned. Attach a note to an instruction, then export the current evidence for someone else to review.',steps:['Scroll down the inspector to <strong>Investigation note</strong>. Write a short explanation and choose <strong>Save note</strong>. Notes are scoped to instruction and example trace.','Use the <strong>Source context</strong> to connect disassembly to the illustrative loop. Source mapping in this prototype is explicitly an example.','Choose <strong>Export view</strong> to download JSON with the selected instruction, cycle view, filters, range, and note. This is an investigation snapshot; it is not a VTR trace export.'],caption:'A saved note next to source context, with Export view available above the workspace.',tip:'Notes stay local. Export is an explicit download; nothing is uploaded.'},
 {id:'commands',title:'Let the keyboard do the traveling',intro:'The command palette makes infrequent actions discoverable and frequent actions fast. Every shortcut also has a pointer-accessible control.',steps:['Open <strong>Commands</strong> with Ctrl/Command + K. Type an action, a cycle number such as <strong>73</strong>, or an instruction ID such as <strong>#1012</strong>.','Use <strong>↑ / ↓</strong> to choose a result, Enter to run it, and Escape to close. The active result is highlighted and kept in view.','Press <strong>?</strong> or the header’s help button for the complete shortcut reference. Shortcuts do not fire while you type into a field.'],caption:'The command palette lists navigation and investigation actions alongside their shortcuts.',tip:'Tab and Shift+Tab remain available throughout; dialogs return focus to their opener.'},
 {id:'appearance',title:'Make the workspace comfortable',intro:'A dense technical tool should adapt to the person using it. Readability and predictable behavior matter more than decoration.',steps:['Switch <strong>light / dark appearance</strong> from the header. Stages retain text labels and squashed work retains its pattern in either theme.','Choose <strong>Compact rows</strong> to see more instructions. The normal density restores secondary PC labels. Narrow windows stack the inspector below the timeline.','Switch between the two top-level tabs with <strong>Left / Right</strong> when a tab has focus. Tutorial screenshots open at full size; each <strong>Try it</strong> button prepares the corresponding example.'],caption:'Dark appearance and compact rows, with the same selection and dependency links.',tip:'Reduced-motion preferences are respected. The app has no runtime downloads or account flow.'}
];
const extraShots={overview:['signals','Choose digital, numeric, analog, and execution-stage signals for the shared time axis.'],search:['search-empty','An empty search explains how to recover without losing the current selection.'],bookmarks:['saved-view','A saved investigation and pinned instruction are available in the sidebar.'],commands:['shortcuts','The complete keyboard reference, available with the help button or question mark.'],appearance:['narrow','At a narrower window width, the inspector moves below the timeline.']};
function extraFigure(id){const extra=extraShots[id];return extra?`<figure class="supplemental-figure"><button class="screenshot-button" data-image="screenshots/${extra[0]}.png" aria-label="Enlarge additional screenshot: ${esc(extra[1])}"><img src="screenshots/${extra[0]}.png" alt="${esc(extra[1])}" loading="lazy" width="1600" height="1000"><span>↗ View full size</span></button><figcaption>${extra[1]}</figcaption></figure>`:'';}
$('guide-links').innerHTML=lessons.map((l,i)=>`<a href="#lesson-${l.id}"><span>${String(i+1).padStart(2,'0')}</span>${l.title.split(':')[0]}</a>`).join('');
$('chapters').innerHTML=lessons.map((l,i)=>`<article class="chapter" id="lesson-${l.id}"><div class="chapter-top"><span class="chapter-number">${String(i+1).padStart(2,'0')}</span><h2>${l.title}</h2></div><p class="chapter-intro">${l.intro}</p><figure><button class="screenshot-button" data-image="screenshots/${l.id}.png" aria-label="Enlarge screenshot: ${l.title}"><img src="screenshots/${l.id}.png" alt="${esc(l.caption)}" loading="lazy" width="1600" height="1000"><span>↗ View full size</span></button><figcaption>${l.caption}</figcaption></figure><ol>${l.steps.map(s=>`<li>${s}</li>`).join('')}</ol>${extraFigure(l.id)}<div class="chapter-footer"><button class="button" data-lesson="${l.id}">Try it in the viewer <span aria-hidden="true">↗</span></button><span class="chapter-tip">${l.tip}</span></div></article>`).join('');
function lesson(id){
 showTab('viewer');reset();state.theme='light';theme();
 switch(id){
 case 'navigate':fit();break;
 case 'inspect':selectInstruction(1012,23,true);break;
 case 'dependencies':selectInstruction(1013,25,true);break;
 case 'search':state.search='ld';state.filter='load';syncControls();renderFilters();renderRows();renderInspector();break;
 case 'flush':loadScenario('branch');filterBy('flush');selectInstruction(1038,73,true);break;
 case 'measure':state.range=[23,41];state.mode='measure';renderRange();renderMetrics();break;
 case 'compare':toggleCompare();break;
 case 'bookmarks':if(!state.pins.includes(1012))togglePin(1012);bookmark();$('bookmark-name').value='First memory stall';break;
 case 'notes':$('inspector').scrollTop=$('inspector').scrollHeight;$('instruction-note').focus({preventScroll:true});break;
 case 'commands':openCommands();break;
 case 'appearance':state.theme='dark';theme();state.compact=true;syncControls();renderRows();sc.scrollTop=7*rowHeight();break;
 }
 window.scrollTo({top:0,behavior:'instant'});
}
$('legend').innerHTML=Object.entries(STAGES).filter(([k])=>!['R','SQ'].includes(k)).map(([key,s])=>`<span title="${s.name}"><i style="background:${s.color}"></i>${key}</span>`).join('');
theme();loadScenario('memory');requestAnimationFrame(focusSelection);
// Hash links are navigation only; trace processing and persistence stay local.
if(location.hash==='#tutorial')showTab('tutorial');
window.addEventListener('hashchange',()=>{if(location.hash==='#viewer')showTab('viewer');else if(location.hash==='#tutorial'||location.hash.startsWith('#lesson-'))showTab('tutorial');});
})();
