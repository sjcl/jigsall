// Optional browser QA. Run with an existing Playwright installation; no gameplay dependency.
'use strict';
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const {chromium}=require(process.env.MATCHING_PLAYWRIGHT_MODULE||'playwright');
const output=path.resolve(process.argv[2]||'target/edge-assessment');
const url=process.argv[3]||'http://127.0.0.1:8785/edge-matching-tool.html';
const html=fs.readFileSync(path.join(output,'edge-matching-tool.html'),'utf8');
const data=JSON.parse(html.match(/<script id="matching-data" type="application\/json">([\s\S]*?)<\/script>/)[1]);
assert.equal(data.length,8);
for(const set of data){
  assert.equal(set.edges.length,64);assert.equal(set.rounds.length,20);
  assert.equal(new Set(set.edges.map(e=>`${e.x},${e.y}`)).size,64);
  for(const round of set.rounds)for(const count of [8,10,12]){
    const candidates=round.candidates[count];assert.equal(candidates.length,count);
    assert.equal(new Set(candidates).size,count);assert.equal(candidates.filter(i=>i===round.target).length,1);
  }
}
const manifest=fs.readFileSync(path.join(output,'random-matching-manifest.csv'),'utf8').trim().split(/\r?\n/).slice(1).map(line=>line.split(','));
for(const seed of new Set(manifest.map(row=>row[0]))){
  const rows=manifest.filter(row=>row[0]===seed);assert.equal(rows.length,32);
  for(const col of [1,2])assert.equal(new Set(rows.map(row=>row[col])).size,32);
  assert.equal(new Set(rows.map(row=>row.slice(3,6).join(','))).size,32);
}
(async()=>{
  const browser=await chromium.launch({headless:true,executablePath:process.env.MATCHING_BROWSER_EXE||undefined});
  try{
    const context=await browser.newContext({viewport:{width:1360,height:1000},acceptDownloads:true});
    const page=await context.newPage();const errors=[];page.on('pageerror',error=>errors.push(String(error)));
    await page.goto(url);await page.getByLabel('参加者名（任意）').fill('QA synthetic');
    await page.getByRole('button',{name:'開始',exact:true}).click();
    await page.waitForFunction(()=>document.querySelectorAll('.candidate:not(:disabled)').length===10);
    const paints=await page.locator('.shape path').evaluateAll(paths=>paths.map(p=>getComputedStyle(p).fill));assert.equal(new Set(paints).size,1);
    const widths=await page.locator('.shape').evaluateAll(svgs=>svgs.map(s=>s.getBoundingClientRect().width));assert.ok(widths.every(w=>Math.abs(w-widths[0])<0.1));
    await page.screenshot({path:path.join(output,'edge-matching-tool.png'),fullPage:true});
    const correct=await page.evaluate(()=>current.candidates.indexOf(current.target));
    await page.getByRole('button',{name:`候補 ${(correct+1)%10+1}`,exact:true}).click();
    assert.equal(await page.evaluate(()=>records.length),1);assert.equal(await page.evaluate(()=>records[0].correct),false);
    await page.getByRole('button',{name:'次の問題',exact:true}).click();
    await page.waitForFunction(()=>phase==='asking');
    const slot=await page.evaluate(()=>current.candidates.indexOf(current.target));
    await page.getByRole('button',{name:`候補 ${slot+1}`,exact:true}).click();
    await page.evaluate(()=>answer(0)); // A repeated callback must not create a second answer.
    const answers=await page.evaluate(()=>records);assert.equal(answers.length,2);assert.equal(answers[1].correct,true);
    assert.ok(answers.every(r=>Number.isFinite(r.response_ms)&&r.response_ms>0&&r.generator_version===5));
    for(const format of ['CSV','JSON']){
      const promise=page.waitForEvent('download');await page.getByRole('button',{name:`${format}を保存`,exact:true}).click();
      const download=await promise;await download.saveAs(path.join(output,`qa-results.${format.toLowerCase()}`));
    }
    assert.equal(JSON.parse(fs.readFileSync(path.join(output,'qa-results.json'),'utf8')).records.length,2);
    const csv=fs.readFileSync(path.join(output,'qa-results.csv'),'utf8');assert.ok(csv.includes('response_ms')&&csv.includes('QA synthetic'));
    await page.reload();assert.equal(await page.evaluate(()=>records.length),2);
    for(const count of ['8','12']){
      await page.getByLabel('候補数').selectOption(count);await page.getByLabel('セルの縦横比').selectOption('1:4');await page.getByLabel('比較する辺').selectOption('V');
      await page.getByRole('button',{name:'開始',exact:true}).click();await page.waitForFunction(n=>document.querySelectorAll('.candidate:not(:disabled)').length===n,Number(count));
      assert.equal(await page.evaluate(()=>session.ratio),4);
      await page.evaluate(()=>{Object.defineProperty(document,'hidden',{configurable:true,value:true});document.dispatchEvent(new Event('visibilitychange'));});
      await page.getByRole('button',{name:'候補 1',exact:true}).click();assert.equal(await page.evaluate(()=>records.at(-1).interrupted),true);
      await page.evaluate(()=>{Object.defineProperty(document,'hidden',{configurable:true,value:false});});
      await page.getByRole('button',{name:'終了して設定に戻る',exact:true}).click();
    }
    const narrow=await browser.newContext({viewport:{width:390,height:844}});const mobile=await narrow.newPage();await mobile.goto(url);
    await mobile.getByRole('button',{name:'開始',exact:true}).click();await mobile.waitForFunction(()=>phase==='asking');
    assert.ok(await mobile.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth));
    await mobile.screenshot({path:path.join(output,'edge-matching-tool-mobile.png'),fullPage:true});await narrow.close();
    assert.deepEqual(errors,[]);console.log('PASS: random fixture manifest, 480 candidate sets, correctness, timing, answer locking, persistence, CSV/JSON exports, 8/10/12 choices, aspect mapping, interrupted trials, mobile layout. Synthetic QA only; no human matching scores.');
  }finally{await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
