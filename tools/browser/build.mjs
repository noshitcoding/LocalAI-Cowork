import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import ts from 'typescript';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const source=path.join(root,'extensions/browser'),identity=JSON.parse(fs.readFileSync(path.join(source,'identity.json'),'utf8'));
const version=JSON.parse(fs.readFileSync(path.join(root,'package.json'),'utf8')).version.split('-')[0];
for(const engine of ['chrome','firefox']){
 const output=path.join(root,'dist/browser-extensions',engine);fs.mkdirSync(output,{recursive:true});
 for(const name of ['policy','background','popup']){
  const code=fs.readFileSync(path.join(source,'src',name+'.ts'),'utf8');
  fs.writeFileSync(path.join(output,name+'.js'),ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext,removeComments:false,newLine:ts.NewLineKind.LineFeed}}).outputText);
 }
 fs.copyFileSync(path.join(source,'popup.html'),path.join(output,'popup.html'));
 const manifest={manifest_version:3,name:'LocalAI Cowork Browser',version,description:'Share selected browser tabs with your local LocalAI Cowork app.',
  permissions:['nativeMessaging','tabs','scripting','activeTab'],optional_host_permissions:['<all_urls>'],
  action:{default_popup:'popup.html',default_title:'LocalAI Cowork'},
  content_security_policy:{extension_pages:"script-src 'self'; object-src 'none'"},
  ...(engine==='chrome'?{minimum_chrome_version:'125',key:identity.key,background:{service_worker:'background.js',type:'module'}}:
   {background:{scripts:['background.js'],type:'module'},browser_specific_settings:{gecko:{id:identity.firefox_id,strict_min_version:'128.0',data_collection_permissions:{required:['websiteContent','websiteActivity'],optional:[]}}}})};
 fs.writeFileSync(path.join(output,'manifest.json'),JSON.stringify(manifest,null,2)+'\n');
}
fs.copyFileSync(path.join(source,'identity.json'),path.join(root,'dist/browser-extensions/identity.json'));
console.log('Chrome and Firefox extensions: dist/browser-extensions');
