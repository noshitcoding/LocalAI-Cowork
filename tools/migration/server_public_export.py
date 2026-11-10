"""Operator-run, read-only PostgreSQL public-data export. Never run automatically.

Install psycopg[binary] in an operator-owned environment. Connection settings use
normal PGHOST/PGPORT/PGDATABASE/PGUSER/PGPASSFILE, never command-line passwords.
No authentication rows, encrypted secrets or execution checkpoints are selected.
"""
import argparse, datetime, decimal, hashlib, json, os, pathlib, stat, sys, uuid

MAX_BYTES=32*1024*1024
MAX_ROWS=10000
# Exact public columns, from the original V1 server migrations 0001..0061.
TABLES={
 'users':'id,revision,email,display_name,deleted_at',
 'teams':'id,revision,name,owner_user_id,deleted_at',
 'team_members':'team_id,user_id,role',
 'projects':'id,revision,owner_user_id,team_id,privacy,name,description,current_version_id,deleted_at',
 'project_members':'project_id,user_id,role',
 'threads':'id,revision,project_id,created_by,title,forked_from_thread_id,forked_from_message_id,created_at,deleted_at',
 'messages':'id,revision,thread_id,author_user_id,role,content,run_id,created_at,deleted_at',
 'task_definitions':'id,revision,project_id,name,instructions,released,created_by,created_at,deleted_at',
 'schedules':'id,revision,project_id,thread_id,task_id,cron_expression,timezone,input,enabled,blocked_reason,created_by,created_at',
 'provider_profiles':'id,revision,owner_user_id,team_id,name,provider_kind,model_defaults,created_at,deleted_at',
 'runs':'id,revision,project_id,thread_id,creator_user_id,state,spec,result,started_at,finished_at,created_at',
 'run_events':'run_id,sequence,event_id,kind,payload,created_at',
 'project_versions':'id,project_id,revision,parent_version_id,merge_base_version_id,snapshot_manifest_id,created_by_user_id,created_by_run_id,created_at',
 'run_artifacts':'id,run_id,revision,kind,media_type,name,size_bytes,created_at,deleted_at',
 'sync_entities':'user_id,entity_type,entity_id,revision,payload,tombstone,created_at',
}
def scalar(value):
 if isinstance(value,(uuid.UUID,datetime.datetime,datetime.date,decimal.Decimal)):return str(value)
 raise ValueError('Unsupported public export value')
def display(value):
 if isinstance(value,str):return value
 if isinstance(value,dict):
  if isinstance(value.get('content'),str):return value['content']
  if isinstance(value.get('text'),str):return value['text']
 if isinstance(value,list):return '\n'.join(x.get('text','') for x in value if isinstance(x,dict) and x.get('type') in ('text','input_text','output_text') and isinstance(x.get('text'),str))
 raise ValueError('Unknown historical message content shape')
def sanitize(table,row):
 if table=='messages':row['content']=display(row['content'])
 if table=='provider_profiles':
  defaults=row.get('model_defaults') or {}
  row['model_defaults']={k:defaults[k] for k in ('base_url','model','temperature','context_window') if k in defaults}
 if table=='runs':
  spec=row.get('spec') or {};inp=spec.get('input') or {}
  row['spec']={'id':str(spec.get('id')),'input':{'prompt':inp.get('prompt',''),'messages':[{'role':m.get('role'),'content':display(m.get('content'))} for m in inp.get('messages',[])]}}
  result=row.get('result');row['result']={'content':result['content']} if isinstance(result,dict) and isinstance(result.get('content'),str) else None
 if table=='run_events':
  payload=row.get('payload') or {}
  row['payload']={k:payload[k] for k in ('content','path','source','storage') if k in payload} if row['kind'] in ('model_completed','artifact_created') else {}
 if table=='schedules':
  inp=row.get('input') or {};row['input']={'prompt':inp.get('prompt','')} if isinstance(inp,dict) else {}
 if table=='sync_entities':
  if row['tombstone'] or row['entity_type'] not in ('skill','memory','crew'):return None
  payload=row.get('payload') or {}
  fields=('id','name','description','project_id','revision','prompt_template','instructions','content','key','scope','scope_ref','agents','tasks','process','managerAgentId')
  payload={k:payload[k] for k in fields if k in payload}
  if isinstance(payload.get('agents'),list):payload['agents']=[{k:a[k] for k in ('id','name','role','goal','backstory','skillsMarkdown','instructions') if k in a} for a in payload['agents']]
  if isinstance(payload.get('tasks'),list):payload['tasks']=[{k:t[k] for k in ('id','description','expectedOutput','agentId','dependencies') if k in t} for t in payload['tasks']]
  row['payload']=payload
 return row
def pairs(items):
 result={}
 for key,value in items:
  if key in result:raise ValueError('Duplicate manifest key')
  result[key]=value
 return result
def selected_plaintext_manifest(selected,result):
 selected=pathlib.Path(selected);assert selected.is_absolute()
 before=selected.lstat();assert stat.S_ISREG(before.st_mode) and before.st_nlink==1 and before.st_size<=1024*1024 and not getattr(before,'st_file_attributes',0)&0x400
 for parent in selected.parents:assert not parent.is_symlink() and not getattr(parent.lstat(),'st_file_attributes',0)&0x400
 with selected.open('rb') as stream:
  held=os.fstat(stream.fileno());assert (held.st_dev,held.st_ino,held.st_size,held.st_mtime_ns)==(before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns)
  raw=stream.read(1024*1024+1);assert len(raw)<=1024*1024
  after=os.fstat(stream.fileno());observed=selected.lstat();assert (after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns)==(before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns)==(observed.st_dev,observed.st_ino,observed.st_size,observed.st_mtime_ns)
 values=json.loads(raw,object_pairs_hook=pairs);assert isinstance(values,dict) and len(values)<=1000
 artifacts={str(x['id']) for x in result['tables'].get('run_artifacts',[])};versions={str(x['id']) for x in result['tables'].get('project_versions',[])}
 total=0
 for key,entry in values.items():
  assert isinstance(key,str) and len(key)<=180 and isinstance(entry,dict) and set(entry)<={'relative_path','size','sha256','name'}
  parts=key.split(':');assert (len(parts)==2 and parts[0]=='artifact' and parts[1] in artifacts) or (len(parts)==3 and parts[0]=='version' and parts[1] in versions and parts[2] and all(c.isalnum() or c in '_-.' for c in parts[2]))
  path=entry['relative_path'];assert isinstance(path,str) and 0<len(path)<=4096 and '\\' not in path and ':' not in path and '\0' not in path
  relative=pathlib.PurePosixPath(path);assert not relative.is_absolute() and '..' not in relative.parts and relative.parts
  size=entry['size'];digest=entry['sha256'];assert type(size) is int and 0<=size<=16*1024*1024 and isinstance(digest,str) and len(digest)==64 and all(c in '0123456789abcdef' for c in digest)
  if 'name' in entry:assert isinstance(entry['name'],str) and 0<len(entry['name'].encode())<=300 and '\0' not in entry['name']
  total+=size;assert total<=32*1024*1024
 return values
def export(output,snapshot_database,decrypted_files_manifest=None):
 import psycopg
 from psycopg.rows import dict_row
 result={'format':'open-cowork-server-public/1','schema_revision':0,'tables':{},'source_migrations':[],'exported_at':datetime.datetime.now(datetime.timezone.utc).isoformat()}
 # The operator chooses the source via libpq configuration. No source credentials
 # are ever included in the exported object or an error message.
 if not snapshot_database or len(snapshot_database)>100 or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-' for c in snapshot_database):raise ValueError('Select one snapshot database')
 with psycopg.connect('',dbname=snapshot_database,row_factory=dict_row,autocommit=True) as connection:
  connection.execute('BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY')
  try:
   connection.execute("SET LOCAL statement_timeout='120s'")
   migrations=connection.execute('SELECT version,success,checksum FROM _sqlx_migrations ORDER BY version').fetchall()
   versions=[int(r['version']) for r in migrations]
   if not versions or versions!=list(range(1,max(versions)+1)) or max(versions)>61 or not all(r['success'] for r in migrations):raise ValueError('Unknown or incomplete V1 server migration profile')
   for r in migrations:
    expected=KNOWN_MIGRATIONS.get(int(r['version']))
    if expected is None or bytes(r['checksum']).hex()!=expected:raise ValueError('Changed V1 server migration checksum')
   result['schema_revision']=max(versions);result['source_migrations']=[{'version':int(r['version']),'checksum':bytes(r['checksum']).hex()} for r in migrations]
   available={r['table_name'] for r in connection.execute("SELECT table_name FROM information_schema.tables WHERE table_schema='public'")}
   for table,columns in TABLES.items():
    if table not in available:continue
    present={r['column_name'] for r in connection.execute('SELECT column_name FROM information_schema.columns WHERE table_schema=\'public\' AND table_name=%s',(table,))}
    columns=','.join(c for c in columns.split(',') if c in present)
    values=[]
    with connection.cursor(name='public_'+table) as cursor:
     cursor.execute('SELECT '+columns+' FROM '+table+' LIMIT '+str(MAX_ROWS+1))
     for row in cursor:
      if len(values)>=MAX_ROWS:raise ValueError('Public export row bound exceeded')
      value=sanitize(table,dict(row))
      if value is not None:values.append(value)
    result['tables'][table]=values
    if len(json.dumps(result,default=scalar,ensure_ascii=False).encode())>MAX_BYTES:raise ValueError('Public export exceeds 32 MiB')
   connection.execute('COMMIT')
  except BaseException:connection.execute('ROLLBACK');raise
 if decrypted_files_manifest:
  result['decrypted_files']=selected_plaintext_manifest(decrypted_files_manifest,result)
 raw=json.dumps(result,default=scalar,ensure_ascii=False,allow_nan=False,separators=(',',':')).encode();assert len(raw)<=MAX_BYTES
 output=pathlib.Path(output);assert output.is_absolute() and not output.exists()
 with output.open('xb') as stream:stream.write(raw);stream.flush();os.fsync(stream.fileno())
 print(json.dumps({'format':result['format'],'schema_revision':result['schema_revision'],'bytes':len(raw),'sha256':hashlib.sha256(raw).hexdigest()}))

# Populated from exact read-only original migration bytes, never from a live DB.
KNOWN_MIGRATIONS={1: '9732a47926235ace4c44e6eb22bd7ea10b582c0cb0c916ae8942a05ce523c85e73d9a4a6a37c4fd900c5ba8da9987f83', 2: 'e9889fd511f4d67308425b83945a09fac462e8028483ffb9674aac603258a64a7706b7da1e5b81faa6e2243105596451', 3: '97e04b8b2542ae28f3f7e2efcad926e886b6174b6534a197c8c178d8806e9a59c0136ef27ad434a488c6ebc8b9a53dab', 4: '808a8f8dd9145ac08e85514c9bdec89d09ddc95ad4dd5b9cbe3591640d2614dfc655245b817acdaf37c5472237656c72', 5: '6131196a1867a71fb4b27be82aa851720f6e0282c6dbb2a29c78104d146c91a737c7292133e465c52deae2ee6facabce', 6: '8f38a3b627f32a370ad48ef27444787b91decb354e477b83bc0dad3a9a294434fbd6073be222e9caf87ec4983188ed27', 7: '3f79127b6bdc633125311313a6e0be5c081c2b284166fd5702a0c3d953009b38b2e85d6e7d4da03aea0170d1ff13e26c', 8: '4a0ffd3488699e48b79293728f853b36f89cea60a4bc352ad672ead2d834968c30f7b91404fdaab9c6d3fde9d2d92d28', 9: 'e6b33c2a6c0f6e1dacc9cfe5a54c3c41956ee99664df86328623ad14a3a7031ac58a8d6382a32163bbe91ccdfdeb4c1d', 10: '1a2c102fce6a30cd2337fd80c922f003f66b7401cd165fca64f982470d126f1fbd6411ba510249a6264435c11a51465e', 11: '1fb4c051f2dbf663a38c6fa22b9f947f444fc290b096a7cf93faf8a801fbdd51ab8cbb89cf991f5a0ca630e6be38dcc5', 12: 'da87774dfd14ee8f72713853bc67c5e7451fab87bed51d8faf33b47f19c91829ec9e837f3d17374ffe077b29af81dbbb', 13: 'aa8ba3730c6dc7d2ae83a3909075b22b6bf08da7f0ad02c095678b960ddafc3e7326446e44c57e9c5582689756a1f269', 14: '37af36d064a07a32ac03199ef6405d630aa45bafe87acf796b8edb8f8a6a93684f80b5847463efd776dfbd73146a717b', 15: '2a3da7c81a393bb448d9d06698a7cee0907a1d88f6c4bbf0bee1b2b7b641456d2ef1870e971f71c4af11b829e2c439ff', 16: '33f496ab1f23ab46f366c46d003123cec933d67d00bc627509dfa6dc5c500b4e8cf38958424f2d7ea14b6d0680a083b2', 17: 'd17c9f34bf2374d82d63187b2cda81a3446cfaae59248ad3c7623b49630e74f56cfe0261b28316d0e37e7e4d91860d42', 18: '615af225f790dac689c58384421474378d69165e1aa21f6fe55260603ce29446b405baa69e4793711c05765e0a8847a6', 19: '1f28484016fbe89db84469e3bfa7ecfa45a80d249139079bc9d96427fa13f90600d50cd3937101a24f5d08c877931264', 20: '740b8c69755ac42aac5ea9b7946f0614bfdfc886ee3190b7bb5f8776b81f177ece61e208c1798b4a561a6461ad09c2e6', 21: '04b4336d97f08b151b3b950ef944169db04af41043c40189f2c056ec0aa633020e91f259cc1e0d6782a7591c416008b3', 22: '531201c52373eaf58614103c59a4bd90e2a1367856f6259ce89d361ad426895da586307c593871db589d0a25f6626cbd', 23: '6fda171c80be7bc6b5d6fc9ad4a3cd7357ffca463cb98a13b2fd730671e7748bf3d2342aa504dc0c9cb93ea1e449c0af', 24: '83591b528147b0ba753712d96fdc1891839ba4789dfc6ce17fc021113fb2da30b954196a76ee6eade3c7051b8fd6c2ae', 25: 'de98ad65847ff8a6f1206a8b3e4a1f293f861d93ef0fc16a92e6432dada421d5e745cc34ddc255c6026737ff780e3cc5', 26: 'bd1a5220e8f7e748dcbdb19d84f34122e2d771aa94f0460ea461199044277f3e2b012be010921c2259e7a11b148c3850', 27: '7e232976e13071c1a715d218d6a654818e410c59e504db9470e742daf1c599d497e3f706242d38def9e16d9e54beab6a', 28: '71fbd54d8bf46df43990bffcdb34a7037788d148821679c032c2de3285b6adfb6a13b4b18522277bfdda9e6a0956156a', 29: 'b677b91c353ca1c4a85898a7a17f528c16f47c8b7fc49f8614ae279fbf2d8156c6f925a37a00bc9632641eff1b55e5bc', 30: '240115df4424df282056a2db8eff76ecf579c7d77f178468afbaaca33926ae74c4a7f83eab81d0feb35243ca14240203', 31: '0b566614c0161aa068a9e10070aab425377c31d3dc13165028f95cffc8d143c88786bb1a0a62d2709fcc30411c19265f', 32: 'cbf110d4f87584408ad671e644fe1a6f5ad62b1473a4d9dbef052e5467dc9e331929cc58d5aad52b089649ee8385bea6', 33: '74fa41775863ad4949dcbeb76d410cb5b4cf2bf6305a78eb9b02381179a09c21db65617388deda1a0e9f8cdef06b1660', 34: '6fee1a67ac22884f0915949d2e06d558d66c5c441babea837daad969bc1a6848db423d5285013e3d89cf4c8bab72ab26', 35: '0aa6ffb53443a86c1e319b51f65935ee22a558b10a765eeb025a000c24ad6b704762e4c9fd3c3c2eb6169e01c6ccd1e8', 36: '0c51d504a91713a50c62a08bbec6f8ef4ff626af6e52ac806c732a34cd6aed490990e62d6dfb357d7d52098c7213c22d', 37: 'ed00f5ded8233317afab12bc4239ad06f6a8c14cc9218155bc5913c3dcc099a2278276638c02cda593d488d43bae9c98', 38: 'b015375093edf45e5c65db6b90001d4372d8b4b2319dd7df9035ac971025db340ae597e1ad192a5df2fe732143569cf2', 39: 'c11ed20641090a963427384396479f13152f7fe9573fc4ccddd885ed85234fdb2d5498308bad6e62aedb08193a3820b7', 40: '46aac481fccd4b9d57b310808eda87fbbe9ef7aee3ce2904927284e30cb4d295e167b215e927c539f92b664e64f252c0', 41: 'ee0892ddc3733b60c8842a9ae1d35449ca6617114fa5ac4f78fbb85ec5cd5ad510527f973f537355c02d63c9af260cd5', 42: '3a383908b0065f619d24f36864c9d392d576fc1fd9dac2fef0c7d669f5ddf4dc167f3369fd15d5251840b2bd91ec942f', 43: '18ff9ae2e6276cd9e497b2f71d05b2e8d9cd347eb621000e92c747e6ae5c23f8038d27a3d5873c071e8dadfe9d807b5a', 44: 'fcfd953370db084f5282d8261728dd91a5f20c752397f8b171e6447c384679aebc4f08df7bc4be5ece7c5ce79d7469f8', 45: '98cdde46043b5d6040415344f52bd2eabd1b8f5f1d1bd68c1c62818278571c9d5640c371f1365983439ec4540b75868a', 46: 'a13b33bed13aa3f652d52e2c079b6e26864c97ca15b23566db8cd2dad9a6019ce9fb0694b6abf89a03468e8fc894670f', 47: '0c2585ba5cc357e58dbdba9dbcd3cefa13e3e2790eee2a17ccc25b651e94f74ca1b2a36ae11db417a8a9c39ea2816480', 48: 'c10d599da84843ba4fa1d42f5edc289c1ddd6404073ca90abbb3ff13baf855c12c11306cab99ae98d4731f0146e7543a', 49: '053633a76b327b3ad0ef87e21f12d2cafae696e3fc7024c077b5e70da1a882d46d8efa413f5aeb0810bc451d8f8a54b0', 50: '896b5c86d01cf234b9b80d15810bee34760dd4748408cc4a92b36e7d2e1464317ca76208436a7148f8715894ed92db3e', 51: '6b9936099b8c9df80c0d7a15162227930a8afcba134b4c7282209dc3ae355f3c61f8aa2d2009a4fa5abd2d409928e784', 52: '37e47e12fa2cbad0e04b0f88577ddd7e2269cdfacb67a3e6bcc92be157846ee7be1b485142ca3bc23d7a5edfe8f555c0', 53: '34eea3a6eefc40cbc10f866ddfe901e2bc7ad22b44a6007f544eee3e90956514a2d2e6c87ccd82ccd1544cefb87a6476', 54: 'd5636dd6bf31088b26b88f3a1af3d041cd0775e82fe2ecaeefeb481811f5a6e7a030d7fdc4223bbfbdb54f2761744a36', 55: 'dc904214967d435b863ecbf2676bc4d83e130991087742712abdb6cf6c63108d8f155f1bfdcacc729789afa4ba85ab87', 56: '16a2b3c150c4467ddf0fed6ceb5798e909527579b5bf1d744eaad158066ed9e89c9ad22544f96bebfd4f6bb4fc90d58c', 57: 'a48efebb832e095ff992689efab47052ab8b2e2f055cd7be2b2572acca0c28f3b8748068c08d2fd36dbb674cff324db3', 58: 'e02171c4bc240cc4efb50b5e77589b503eb3a4441a2560583d88b10b6f9ffa5b74ccf3ec829f88bb47fa34107ef04760', 59: '7b6aab2ecdca91101b0609d50a08a5ea5becec239950ed9844483f3f97aa07d2a01dd764bb8dcebfa78339aee8f6c7f2', 60: '473f4e1edd96dbd7c86431384034a8a46149c465f37bb31cba9b72f99a1a4ae37b099f66c234c98b694becf17352fc7d', 61: '7e1227378d4c7f0d4e2d1db9d190eaff211eec0aa2651cbef1fc3e6b51a6838d14fe99b92008d231d3d3037b01741296'}
if __name__=='__main__':
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',required=True);parser.add_argument('--snapshot-database',required=True);parser.add_argument('--offline-snapshot-confirmed',action='store_true',required=True);parser.add_argument('--decrypted-files-manifest');args=parser.parse_args()
 try:export(args.output,args.snapshot_database,args.decrypted_files_manifest)
 except Exception:
  print('V1 public export failed; no source credentials or row contents are printed',file=sys.stderr);sys.exit(1)
