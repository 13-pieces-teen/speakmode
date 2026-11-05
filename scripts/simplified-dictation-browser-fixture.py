"""Build an isolated manual/CUA QA site around the current production bundle.

No browser automation, microphone, credentials or native clipboard/input.
Run after npm --prefix app run build, then serve the printed directory locally.
"""
import ast
import json
import re
import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / '.validation' / 'simplified-ui'
OUT.mkdir(parents=True, exist_ok=True)
# Include public fonts/illustrations too: layout QA must use the packaged assets,
# not silently fall back to fonts installed on the test machine.
shutil.copytree(ROOT / 'app/dist', OUT, dirs_exist_ok=True)
index = (ROOT / 'app/dist/index.html').read_text(encoding='utf-8')
assets = re.findall(r'<(?:script|link)\b[^>]*(?:src|href)="\./assets/[^\"]+"[^>]*(?:></script>|>)', index)
bundle = re.search(r'src="(\./assets/[^\"]+\.js)"', index)[1]
styles = ''.join(tag for tag in assets if tag.startswith('<link'))
errors = """<script>window.addEventListener('error',event=>{const report=()=>{const node=document.createElement('pre');node.dataset.uiError='true';node.textContent=[event.message,event.filename,event.lineno,event.error?.stack].join('\\n');document.body.append(node);};if(document.body)report();else window.addEventListener('DOMContentLoaded',report,{once:true});});</script>"""
fixture = (ROOT / 'scripts/compact-recorder-browser-fixture.html').read_text(encoding='utf-8')
fixture = fixture.replace('</head>', styles + errors + '</head>').replace("await import('/src/main.tsx');", f'await import({json.dumps(bundle)});')
(OUT / 'compact-recorder-browser-fixture.html').write_text(fixture, encoding='utf-8')
lab = (ROOT / 'scripts/compact-recorder-browser-lab.html').read_text(encoding='utf-8')
buttons = ''.join(f'<button data-scenario="{key}">{label}</button>' for key,label in [('processing','处理'),('complete','正常完成'),('failed','不完整识别'),('partial','部分输入'),('fallback','整理回退')])
lab = lab.replace('</header>', buttons + '<a href="main.html" target="_blank">结果详情</a></header>').replace('<style>',errors+'<style>',1)
lab = lab.replace("const frame=document", "document.querySelectorAll('[data-scenario]').forEach(button=>button.onclick=()=>send({type:'scenario',value:button.dataset.scenario}));const frame=document")
lab = lab.replace("if(p.type==='metrics')", "if(p.type==='openResult')window.open('main.html'+(p.recovery?'?recovery=1':''),'_blank');if(p.type==='dismissed')document.getElementById('metrics').dataset.dismissed='true';if(p.type==='metrics')")
lab = lab.replace('width:336px;height:66px','width:298px;height:68px')
(OUT / 'index.html').write_text(lab, encoding='utf-8')

# Reuse only pure fixture declarations from the existing regression harness.
tree = ast.parse((ROOT / 'scripts/restore-delivery-browser-qa.py').read_text(encoding='utf-8'))
nodes = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'fixture' or isinstance(node, ast.Assign) and any(isinstance(t,ast.Name) and t.id in ('NOW','BRIDGE') for t in node.targets)]
scope = {}
exec(compile(ast.Module(body=nodes,type_ignores=[]), '<fixture-declarations>', 'exec'), scope)
data = scope['fixture']('unconfirmed','inputDispatchedUnconfirmed')
data['snapshot']['mainResultRequest'] = {'requestId':'open-current','sessionId':'synthetic-session','generation':1,'target':'result'}
data['snapshot']['preferences'].update(recorderCaptionsEnabled=False,softVoiceEnabled=False)
data['detail']['output']['blocks'][0]['text'] = '明天下午讨论 SpeakMood 的交互设计，确认之后再开始开发。'
bridge = scope['BRIDGE'].replace('__FIXTURE__', json.dumps(data,ensure_ascii=False))
bridge = bridge.replace('let callbackId = 0, revision = 1;', '''let callbackId = 0, revision = 1;
 if(location.search.includes('recovery')){fixture.snapshot.mainResultRequest.target='recovery';Object.assign(fixture.snapshot.session,{state:'incomplete',error:'识别未完整，已有文字仍保留。',canCopyRecovery:true});}
''')
bridge = bridge.replace("if(name==='audio.listDevices')return [];", '''if(name==='audio.listDevices')return [];
   if(name==='app.ackMainResult'){fixture.snapshot.mainResultRequest=null;return null;}
   if(name==='personalization.get')return {version:1,enabled:false,activeProfileId:null,profiles:[],preferences:{sentenceStyle:'default',tone:'natural',keepEnglishTerms:false,fewerCourtesies:false,paragraphs:false,lists:false},sceneOverrides:[]};
   if(name==='usage.today'||name==='usage.total')return copy(fixture.snapshot.usage);
   if(name==='recoverable.copy')return null;
   if(name==='preferences.update'){Object.assign(fixture.snapshot.preferences,payload.patch);fixture.snapshot.preferences.version++;return copy(fixture.snapshot.preferences);}
   if(name==='revision.correctTerm'){
    const d=fixture.detail,o=d.output,p=payload.patch;
    if(payload.expectedCurrentRevisionId!==o.id)throw Error('Synthetic stale revision');
    const b=o.blocks.find(b=>b.id===p.blockId),chars=Array.from(b.text);
    if(chars.slice(p.start,p.end).join('')!==p.expectedText)throw Error('Synthetic selection changed');
    b.text=chars.slice(0,p.start).join('')+p.replacement+chars.slice(p.end).join('');o.id='synthetic-corrected';d.session.currentOutputId=o.id;d.canUndo=true;
    return {detail:copy(d),dictionary:null,dictionaryStatus:'notRequested'};
   }''')
main = index.replace('<head>', '<head>'+errors+'<script>'+bridge+'</script>')
(OUT / 'main.html').write_text(main, encoding='utf-8')
print(OUT)
