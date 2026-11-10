/** Project intents and content scopes share one transaction domain. */
export async function replicaDatabase():Promise<IDBDatabase>{
 return new Promise((resolve,reject)=>{
  let rejected=false;
  const request=indexedDB.open("cowork-team-replicas",2);
  request.onupgradeneeded=()=>{
   if(rejected){request.transaction?.abort();return;}
   const db=request.result;
   if(!db.objectStoreNames.contains("replicas"))db.createObjectStore("replicas",{keyPath:"scope"});
   if(!db.objectStoreNames.contains("project_creations"))db.createObjectStore("project_creations",{keyPath:"scope"});
   if(!db.objectStoreNames.contains("workspaces"))db.createObjectStore("workspaces",{keyPath:"scope"});
  };
  request.onsuccess=()=>{if(rejected){request.result.close();return;}request.result.onversionchange=()=>request.result.close();resolve(request.result);};
  request.onerror=()=>{rejected=true;reject(new Error("offline_project_storage"));};
  request.onblocked=()=>{rejected=true;reject(new Error("offline_project_upgrade_blocked"));};
 });
}
