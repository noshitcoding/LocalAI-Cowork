// Only this RP origin creates credentials. Unknown Finish effects retain one original request.
const q=new URLSearchParams(location.search),ticket=new URLSearchParams(location.hash.slice(1)).get('ticket');
if(ticket)history.replaceState(null,'',location.pathname+location.search);
const marker='cowork/native-passkey/pending-v1',form=document.getElementById('login'),status=document.getElementById('status');
let pending=null,busy=false,unresolved=false;
try{unresolved=sessionStorage.getItem(marker)==='pending';}catch{unresolved=true;}
const decode=s=>Uint8Array.from(atob(s.replace(/-/g,'+').replace(/_/g,'/')),c=>c.charCodeAt(0));
const encode=b=>btoa(String.fromCharCode(...new Uint8Array(b))).replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,'');
async function request(path,body){const r=await fetch('/api/v1/auth/native/passkey/'+path,{method:'POST',redirect:'error',cache:'no-store',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});if(!r.ok)throw Error('Passkey-Anfrage fehlgeschlagen / Passkey request failed');return r.json();}
function showPending(){document.getElementById('abandon').hidden=!unresolved;document.querySelector('#login button').textContent=pending?'Originalantwort erneut anfordern / Retry original response':ticket?'Passkey bestätigen / Confirm passkey':'Anmelden / Sign in';}
function callback(finish){const state=q.get('state'),redirect=q.get('redirect_uri')||'open-cowork://auth/callback';const target=new URL(finish.redirect_url),expected=new URL(redirect);if(target.origin!==expected.origin||target.protocol!==expected.protocol||target.hostname!==expected.hostname||target.port!==expected.port||target.pathname!==expected.pathname||target.searchParams.getAll('state').length!==1||target.searchParams.get('state')!==state||target.searchParams.getAll('code').length!==1||!/^cw2a_[0-9a-f]{64}$/.test(target.searchParams.get('code')||''))throw Error('Ungültiges Callback / Invalid callback');
 // Keep the marker until navigation succeeds; repeated navigation uses the same original receipt.
 location.assign(target.href);
}
async function finishOriginal(){for(let attempt=0;attempt<2;attempt++){try{const result=await request(pending.path,pending.body);pending.reply=result;callback(result);return;}catch(error){if(attempt===1)throw error;}}}
form.addEventListener('submit',async e=>{e.preventDefault();if(busy)return;busy=true;try{
 if(pending){if(pending.reply)callback(pending.reply);else await finishOriginal();return;}
 if(unresolved)throw Error('Dieser Browser hat eine unbestätigte Originalanfrage. Ohne ursprüngliche Daten wird kein neuer Passkeydialog geöffnet. / This browser has an unconfirmed original request. A new passkey dialog requires explicitly removing its local marker.');
 const state=q.get('state'),redirect=q.get('redirect_uri')||'open-cowork://auth/callback';let path,body;
 if(ticket){
  const start=await request('register/options',{ticket});if(start.state!==state||start.redirect_uri!==redirect)throw Error('Ungültige Registrierungsbindung / Invalid registration binding');
  const options=start.public_key.publicKey||start.public_key;options.challenge=decode(options.challenge);options.user={...options.user,id:decode(options.user.id)};options.excludeCredentials=(options.excludeCredentials||[]).map(x=>({...x,id:decode(x.id)}));
  status.textContent='Passkey bestätigen für / Confirm passkey for '+options.user.name+' · '+start.label;
  const c=await navigator.credentials.create({publicKey:options});if(!c)throw Error('Kein Passkey erstellt / No passkey created');
  path='register/finish';body={ticket,credential:{id:c.id,rawId:encode(c.rawId),type:c.type,response:{attestationObject:encode(c.response.attestationObject),clientDataJSON:encode(c.response.clientDataJSON),transports:c.response.getTransports?c.response.getTransports():[]},extensions:c.getClientExtensionResults()}};
 }else{
  const start=await request('start',{email:document.getElementById('email').value,device_id:q.get('device_id'),code_challenge:q.get('code_challenge'),code_challenge_method:'S256',state,redirect_uri:redirect});
  const options=start.public_key.publicKey;options.challenge=decode(options.challenge);options.allowCredentials=(options.allowCredentials||[]).map(x=>({...x,id:decode(x.id)}));
  const c=await navigator.credentials.get({publicKey:options});if(!c)throw Error('Kein Passkey ausgewählt / No passkey selected');
  path='finish';body={challenge_id:start.challenge_id,authorization_id:start.authorization_id,credential:{id:c.id,rawId:encode(c.rawId),type:c.type,response:{authenticatorData:encode(c.response.authenticatorData),clientDataJSON:encode(c.response.clientDataJSON),signature:encode(c.response.signature),userHandle:c.response.userHandle?encode(c.response.userHandle):null},extensions:c.getClientExtensionResults()}};
 }
 sessionStorage.setItem(marker,'pending');unresolved=true;pending={path,body};showPending();await finishOriginal();
}catch(error){status.textContent=error instanceof Error?error.message:'Anmeldung fehlgeschlagen / Sign-in failed';}finally{busy=false;showPending();}});
document.getElementById('abandon').addEventListener('click',()=>{if(busy||!confirm('Lokalen Marker ausdrücklich entfernen? Ein weiterer Passkeydialog startet einen neuen Vorgang; bestätigte Serverwirkungen bleiben bestehen. / Explicitly remove the local marker? Another dialog starts a new operation; confirmed server effects remain.'))return;try{sessionStorage.removeItem(marker);pending=null;unresolved=false;status.textContent='Lokaler Marker entfernt / Local marker removed';showPending();}catch{status.textContent='Lokaler Speicher nicht verfügbar / Local storage unavailable';}});
if(ticket){document.querySelector('h1').textContent='Passkey hinzufügen / Add passkey';document.getElementById('email').required=false;document.getElementById('email').closest('label').hidden=true;}
if(unresolved)status.textContent='Unbestätigte Originalanfrage: ursprüngliche Antwort erneut anfordern oder lokalen Marker ausdrücklich entfernen. / Unconfirmed original request: retry the original response or explicitly remove its local marker.';
showPending();
