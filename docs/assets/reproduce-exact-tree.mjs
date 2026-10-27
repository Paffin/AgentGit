// SPDX-License-Identifier: Apache-2.0
import {spawn,spawnSync,execFileSync} from 'node:child_process'
import {mkdtempSync,readFileSync,writeFileSync,mkdirSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {pathToFileURL} from 'node:url'
const sourceRoot=process.env.AG_SOURCE_ROOT??process.cwd()
const binaryRoot=process.env.AG_BIN_DIR??join(sourceRoot,'target/debug')
const {chromium}=await import(pathToFileURL(join(sourceRoot,'apps/web/node_modules/playwright/index.mjs')).href)
const root=mkdtempSync(join(tmpdir(),'agentgit-stale-')),data=join(root,'server'),out=process.env.AG_DEMO_OUTPUT??join(process.cwd(),'agentgit-demo');mkdirSync(out,{recursive:true})
const ag=join(binaryRoot,'ag'),worker=join(binaryRoot,'ag-worker')
const server=spawn(join(binaryRoot,'ag-server'),['dev','--port','0','--data-dir',data],{stdio:['ignore','pipe','pipe']})
let banner='',browser;server.stdout.on('data',b=>banner+=b.toString());const pause=ms=>new Promise(r=>setTimeout(r,ms))
const fast=process.env.DEMO_FAST==='1',hold=ms=>pause(fast?100:ms)
try {
 for(let i=0;i<100&&!banner.match(/http:\/\/127\.0\.0\.1:\d+/);i++)await pause(100)
 const url=banner.match(/http:\/\/127\.0\.0\.1:\d+/)?.[0];if(!url)throw Error('No server URL')
 const token=readFileSync(join(data,'dev/token'),'utf8').trim(),env={...process.env,AG_CONFIG:join(root,'config'),AG_TOKEN:token}
 const run=(args,cwd=root,extra={},expected=0)=>{const r=spawnSync(ag,args,{cwd,env:{...env,...extra},encoding:'utf8',timeout:30000});if(r.status!==expected)throw Error(`ag ${args[0]}: expected ${expected}, got ${r.status}: ${r.stderr}`);return r.stdout}
 const api=async(path,body)=>{const r=await fetch(url+path,{method:'POST',headers:{Authorization:`Bearer ${token}`,'Content-Type':'application/json'},body:JSON.stringify(body)});const j=await r.json();if(!r.ok)throw Error(JSON.stringify(j));return j}
 await api('/v1/roles',{subject:'service:demo-ci',repo:'demo/welcome',role:'runner'})
 const reviewer=readFileSync(join(data,'dev/reviewer-token'),'utf8').trim()
 const review=(id,cwd)=>run(['remote','review',id,'--approve'],cwd,{AG_TOKEN:reviewer})
 const ci=(await api('/v1/credentials',{subject:'service:demo-ci',ttl_secs:3600,description:'local real-worker demo'})).token
 const key=join(root,'ci.key'),keyOutput=execFileSync(worker,['keygen','--out',key,'--identity','service:demo-ci'],{encoding:'utf8'})
 const pub=keyOutput.match(/"public_key"\s*:\s*"([^"]+)"/)?.[1];if(!pub)throw Error('No public signing key')
 run(['clone',url+'/demo/welcome','a']);run(['clone',url+'/demo/welcome','b']);const a=join(root,'a'),b=join(root,'b')
 const policy=join(root,'policy.json');writeFileSync(policy,JSON.stringify({approvals:{min:1},trusted_issuers:[{identity:'service:demo-ci',public_key:pub}],required_validations:[{check:'tests',issuers:['service:demo-ci'],run:{argv:['/bin/sh','-c','test -f README.md && test -f src/greeting.py'],timeout_secs:30}}]}));run(['remote','policy','main','--set',policy],a)
 const validate=async(id,cwd)=>{run(['remote','prepare',id,'--validate'],cwd,{},3);await new Promise((resolve,reject)=>{const p=spawn(worker,['run','--server',url,'--key',key,'--repo','demo/welcome','--once','--sandbox','namespace','--wait-secs','1'],{env:{...env,AG_TOKEN:ci},stdio:['ignore','pipe','pipe']});let log='';p.stdout.on('data',x=>log+=x);p.stderr.on('data',x=>log+=x);p.on('exit',code=>code===0?resolve():reject(Error('worker: '+log)));});const e=JSON.parse(run(['--json','remote','explain',id],cwd));if(e.standing!=='ready')throw Error('Worker did not make proposal ready')}
 browser=await chromium.launch();const context=await browser.newContext({viewport:{width:1440,height:1000},recordVideo:{dir:join(out,'stale-recordings'),size:{width:1440,height:1000}}});const page=await context.newPage();await page.addInitScript(t=>sessionStorage.setItem('agentgit.token',t),token)
 const scene=async(path,title,detail)=>{await page.goto(url+path);await page.getByRole('banner').waitFor();await page.evaluate(({title,detail})=>{let box=document.createElement('aside');box.setAttribute('data-demo-caption','true');box.style.cssText='position:fixed;bottom:18px;left:24px;right:24px;padding:18px 24px;background:#112d28;color:white;z-index:99999;border-radius:6px;box-shadow:0 3px 18px #0003;font:16px/1.5 system-ui';const h=document.createElement('strong');h.style.cssText='display:block;font-size:20px';h.textContent=title;const p=document.createElement('div');p.textContent=detail;box.append(h,p);document.body.append(box)},{title,detail})}
 await scene('/repos/demo/welcome','Exact-tree validation · AgentGit rc.3','Real server and namespace worker. Two scripted workspaces; no model output or independent pilot is represented.');await hold(20000)
 writeFileSync(join(a,'README.md'),readFileSync(join(a,'README.md'),'utf8')+'\nWorkspace A documents the project.\n');run(['snapshot','-m','Document the project'],a);const pa=JSON.parse(run(['--json','push','-t','Workspace A: document project'],a)).proposal.id
 review(pa,a);await validate(pa,a);await scene(`/repos/demo/welcome/proposals/${pa}`,'1 · A passes the real check','A separate reviewer approves the proposal. The namespace worker checks real files and signs the exact candidate tree.');await hold(30000)
 mkdirSync(join(b,'docs'),{recursive:true});writeFileSync(join(b,'docs','parallel.md'),'A concurrent, non-conflicting change.\n');run(['snapshot','-m','Concurrent documentation'],b);const pb=JSON.parse(run(['--json','push','-t','Workspace B: concurrent documentation'],b)).proposal.id;review(pb,b);await validate(pb,b);run(['remote','integrate',pb],b)
 await scene(`/repos/demo/welcome/proposals/${pb}`,'2 · B lands a concurrent change','B changes a different file. The target stream advances, so A now has a different candidate tree.');await hold(30000)
 const blocked=JSON.parse(run(['--json','remote','explain',pa],a,{},3));if(blocked.standing!=='blocked')throw Error('Expected A to lose validation after target moved')
 await scene(`/repos/demo/welcome/proposals/${pa}`,'3 · The old pass does not authorize A','The earlier check still describes the old tree. A is blocked until the new combined candidate is checked.');await hold(35000)
 await validate(pa,a);await scene(`/repos/demo/welcome/proposals/${pa}`,'4 · Validate the new candidate','The real worker runs again. The new signed pass now covers the tree that will actually land.');await hold(25000)
 run(['remote','integrate',pa],a);run(['pull'],a);const mirror=join(root,'mirror.git');run(['git','export','main',mirror],a);const exported=execFileSync('git',['-C',mirror,'show','main:docs/parallel.md'],{encoding:'utf8'});if(!exported.includes('non-conflicting'))throw Error('Git export missing concurrent file')
 await scene(`/repos/demo/welcome/proposals/${pa}`,'5 · Land and retain an exit path','Both changes are integrated. A real Git export contains the concurrent file; reviews, policies and audit require an AgentGit backup.');await hold(35000)
 const video=page.video();await context.close();await video.saveAs(join(out,fast?'stale-check.webm':'agentgit-exact-tree.webm'))
 writeFileSync(join(out,'stale-demo-evidence.json'),JSON.stringify({version:run(['--version']).trim(),proposal_a:pa,proposal_b:pb,old_pass_rejected:true,namespace_worker_real_checks:true,revalidated_and_integrated:true,git_export_verified:true,scope:'Two scripted workspaces operated by one person; not live model execution or an external pilot.'},null,2));console.log('Exact-tree demonstration passed, recording saved.')
}finally{await browser?.close();server.kill('SIGTERM')}
