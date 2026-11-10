"""Generate SPDX 2.3 from locked Rust/npm dependency graphs and delivered bytes.

No upload, package installation or secret discovery is performed. Artifacts must
be selected explicitly. Unknown third-party licenses remain NOASSERTION rather
than being invented. This inventory is not a security or license approval.
"""
from __future__ import annotations
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import importlib.util
import os
import tomllib
import uuid
from urllib.parse import quote

_spec = importlib.util.spec_from_file_location('cowork_runtime_inventory', Path(__file__).with_name('runtime_inventory.py'))
runtime = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(runtime)


def identifier(value):
    return 'SPDXRef-'+hashlib.sha256(value.encode()).hexdigest()[:32]


def digest(path):
    value=hashlib.sha256()
    with path.open('rb') as source:
        while data:=source.read(1024*1024):value.update(data)
    return value.hexdigest()


def strict_json(raw):
    if len(raw)>64*1024**2:raise ValueError('SBOM JSON exceeds bound')
    def pairs(items):
        value={}
        for key,item in items:
            if key in value:raise ValueError('Duplicate SBOM field')
            value[key]=item
        return value
    return json.loads(raw,object_pairs_hook=pairs)


def locked_graph(path):
    """Complete source lock graph, including build/dev/other-target packages.

    This is deliberately not advertised as binary linkage. No network, cache
    installation or target execution is needed to read a release's source graph.
    """
    lock=tomllib.loads(runtime.ordinary(path).read_text(encoding='utf-8'))
    packages=lock.get('package',[]);names={};by_key={}
    for item in packages:
        key=(item['name'],item['version'],item.get('source',''))
        if key in by_key:raise ValueError('Duplicate Cargo lock identity')
        by_key[key]=item;names.setdefault(item['name'],[]).append(key)
    graph={'packages':[],'resolve':{'nodes':[]}}
    for key,item in by_key.items():
        package=dict(item,id=repr(key),source=item.get('source'))
        graph['packages'].append(package);dependencies=[]
        for value in item.get('dependencies',[]):
            match=re.fullmatch(r'([^ ]+)(?: ([^ ]+))?(?: \((.+)\))?',value)
            if not match:raise ValueError('Unsupported Cargo dependency identity')
            name,version,source=match.groups()
            candidates=[key for key in names.get(name,[]) if (version is None or key[1]==version) and (source is None or key[2]==source)]
            if len(candidates)!=1:raise ValueError('Ambiguous Cargo lock dependency')
            dependencies.append(repr(candidates[0]))
        graph['resolve']['nodes'].append({'id':repr(key),'dependencies':dependencies})
    return graph


def add_runtime(report,inventory,prefix):
    if inventory.get('format')!=runtime.FORMAT or inventory.get('namespace_stable') is not True or inventory.get('executed_payload') is not False:
        raise ValueError('Original inspected runtime inventory required')
    files=inventory.get('files');components=inventory.get('components',[])
    if not isinstance(files,dict) or len(files)>runtime.MAX_FILES:raise ValueError('Invalid runtime file inventory')
    parent=identifier('runtime:'+prefix+':'+runtime.canonical_digest(files))
    package={'SPDXID':parent,'name':inventory['name'],'downloadLocation':'NOASSERTION','filesAnalyzed':False,
             'licenseConcluded':'NOASSERTION','licenseDeclared':'NOASSERTION','copyrightText':'NOASSERTION',
             'primaryPackagePurpose':'APPLICATION','sourceInfo':'Actual selected runtime namespace SHA256 '+runtime.canonical_digest(files)}
    if inventory.get('version'):package['versionInfo']=inventory['version']
    report['packages'].append(package)
    relationships=report['relationships']
    def relation(source,kind,target):relationships.append({'spdxElementId':source,'relationshipType':kind,'relatedSpdxElement':target})
    relation('SPDXRef-DOCUMENT','DESCRIBES',parent)
    ids={};native={}
    for name,record in sorted(files.items()):
        runtime.relative(name)
        if not re.fullmatch('[0-9a-f]{64}',record.get('sha256','')):raise ValueError('Invalid runtime file SHA256')
        sid=identifier(prefix+'/'+name);ids[name]=sid
        value={'SPDXID':sid,'fileName':'./'+prefix+'/'+name,'checksums':[{'algorithm':'SHA256','checksumValue':record['sha256']}],
               'licenseConcluded':'NOASSERTION','licenseInfoInFiles':['NOASSERTION'],'copyrightText':'NOASSERTION'}
        if 'link' in record:value['comment']='Symlink namespace; SHA256 covers link text, not target bytes: '+record['link']
        report['files'].append(value);relation(parent,'CONTAINS',sid)
        if record.get('native'):native.setdefault(Path(name).name.lower(),[]).append(sid)
    component_ids={}
    for number,component in enumerate(components):
        name=component['name'];version=component.get('version');sid=identifier(prefix+':component:'+str(number))
        normalized=re.sub('[-_.]+','-',name).lower()
        if normalized in component_ids:raise ValueError('Duplicate runtime component identity')
        component_ids[normalized]=sid
        value={'SPDXID':sid,'name':name,'downloadLocation':component.get('url','NOASSERTION'),'filesAnalyzed':False,
               'licenseConcluded':'NOASSERTION','licenseDeclared':component.get('license') or 'NOASSERTION','copyrightText':'NOASSERTION'}
        if version:value['versionInfo']=version
        purl=component.get('purl') or 'pkg:'+component.get('ecosystem','generic')+'/'+quote(name.lower(),safe='/')+('@'+quote(version,safe='') if version else '')
        value['externalRefs']=[{'referenceCategory':'PACKAGE-MANAGER','referenceType':'purl','referenceLocator':purl}]
        if component.get('sha256'):value['checksums']=[{'algorithm':'SHA256','checksumValue':component['sha256']}]
        if component.get('source'):value['sourceInfo']=component['source']
        report['packages'].append(value);relation(parent,'CONTAINS',sid)
        for filename in component.get('files',[]):
            if filename not in ids:raise ValueError('Runtime component claims missing file')
            relation(sid,'CONTAINS',ids[filename])
        if component.get('metadata'):
            filename=component['metadata']
            if filename not in files or files[filename]['sha256']!=component['metadata_sha256']:raise ValueError('Python metadata binding differs')
            relation(sid,'GENERATED_FROM',ids[filename])
    for component in components:
        source=component_ids[re.sub('[-_.]+','-',component['name']).lower()]
        for dependency in component.get('requires',[]):
            target=component_ids.get(re.sub('[-_.]+','-',dependency['name']).lower())
            if target:relation(source,'DEPENDS_ON',target)
    external={};unresolved=[]
    for name,record in sorted(files.items()):
        if not record.get('native'):continue
        for dependency in record['native']['imports']:
            candidates=native.get(dependency.lower(),[])
            if len(candidates)==1:target=candidates[0]
            else:
                key=(record['native']['format'],dependency)
                if key not in external:
                    target=identifier('native-requirement:'+repr(key));external[key]=target
                    report['packages'].append({'SPDXID':target,'name':dependency,'downloadLocation':'NOASSERTION','filesAnalyzed':False,
                       'licenseConcluded':'NOASSERTION','licenseDeclared':'NOASSERTION','copyrightText':'NOASSERTION',
                       'sourceInfo':'Observed native import; operator OS/runtime requirement. Not a claim of bundled or installed dependency bytes.'})
                target=external[key];unresolved.append({'file':prefix+'/'+name,'library':dependency,'bundled_candidates':len(candidates)})
            relation(ids[name],'DYNAMIC_LINK',target)
    return {'name':inventory['name'],'files':len(files),'components':len(components),'namespace_sha256':runtime.canonical_digest(files),'external_imports':unresolved}


def generate(root,artifacts,*,payloads=(),runtime_inventories=(),include_desktop=True,include_npm=True,include_cargo=True,upstream_documents=()):
    root=runtime.ordinary(root,directory=True)
    locks=[root/'Cargo.lock'] if include_cargo else []
    if include_cargo and include_desktop and (root/'apps/desktop/src-tauri/Cargo.lock').exists():locks.append(root/'apps/desktop/src-tauri/Cargo.lock')
    lock_pins={path:runtime.pin(path) for path in locks}
    graphs=[locked_graph(path) for path in locks]
    graph={'packages':list({item['id']:item for g in graphs for item in g['packages']}.values()),
           'resolve':{'nodes':list({item['id']:item for g in graphs for item in g['resolve']['nodes']}.values())}}
    packages=[];relationships=[];ids={}
    for item in graph['packages']:
        key=item['id'];sid=identifier('cargo:'+key);ids[key]=sid
        package={'SPDXID':sid,'name':item['name'],'versionInfo':item['version'],
                 'downloadLocation':'NOASSERTION','filesAnalyzed':False,
                 'licenseConcluded':'NOASSERTION','licenseDeclared':item.get('license') or 'NOASSERTION',
                 'copyrightText':'NOASSERTION','externalRefs':[{'referenceCategory':'PACKAGE-MANAGER',
                 'referenceType':'purl','referenceLocator':'pkg:cargo/'+item['name']+'@'+item['version']}],
                 'sourceInfo':'Locked source package; includes build/dev/other-target packages. Binary linkage not asserted.'}
        if item.get('checksum'):package['checksums']=[{'algorithm':'SHA256','checksumValue':item['checksum']}]
        packages.append(package)
        relationships.append({'spdxElementId':'SPDXRef-DOCUMENT','relationshipType':'DESCRIBES','relatedSpdxElement':sid})
    for node in (graph.get('resolve') or {}).get('nodes',[]):
        for dependency in node.get('dependencies',[]):
            relationships.append({'spdxElementId':ids[node['id']],'relationshipType':'DEPENDS_ON','relatedSpdxElement':ids[dependency]})
    npm_path=root/'package-lock.json'
    if include_npm:lock_pins[npm_path]=runtime.pin(npm_path)
    lock=strict_json(npm_path.read_bytes()) if include_npm else {'name':'localai-cowork-v2','packages':{}}
    npm_ids={}
    for location,item in lock.get('packages',{}).items():
        if item.get('link'):continue
        name=item.get('name') or (location.rsplit('node_modules/',1)[-1] if 'node_modules/' in location else lock['name'])
        version=item.get('version')
        if not version:
            manifest=root/location/'package.json' if location else root/'package.json'
            metadata=strict_json(runtime.ordinary(manifest).read_bytes());name=metadata['name'];version=metadata['version']
        sid=identifier('npm:'+location+':'+version);npm_ids[location]=sid
        package={'SPDXID':sid,'name':name,'versionInfo':version,'downloadLocation':item.get('resolved','NOASSERTION'),
                 'filesAnalyzed':False,'licenseConcluded':'NOASSERTION','licenseDeclared':item.get('license') or 'NOASSERTION',
                 'copyrightText':'NOASSERTION','externalRefs':[{'referenceCategory':'PACKAGE-MANAGER','referenceType':'purl',
                 'referenceLocator':'pkg:npm/'+quote(name,safe='/')+'@'+quote(version,safe='')}]}
        if item.get('integrity'):
            package['sourceInfo']='npm lock integrity: '+item['integrity']
        packages.append(package)
        relationships.append({'spdxElementId':'SPDXRef-DOCUMENT','relationshipType':'DESCRIBES','relatedSpdxElement':sid})
    for location,item in lock.get('packages',{}).items():
        if item.get('link'):
            if item.get('resolved') not in npm_ids:raise ValueError('Unresolved npm workspace link')
            npm_ids[location]=npm_ids[item['resolved']]
    for location,item in lock.get('packages',{}).items():
        if item.get('link'):continue
        for name in {*item.get('dependencies',{}),*item.get('devDependencies',{}),*item.get('optionalDependencies',{})}:
            # npm resolves from the nearest enclosing node_modules directory.
            parent=location
            while True:
                candidate=(parent+'/' if parent else '')+'node_modules/'+name
                if candidate in npm_ids:
                    relationships.append({'spdxElementId':npm_ids[location],'relationshipType':'DEPENDS_ON','relatedSpdxElement':npm_ids[candidate]});break
                if not parent:
                    if name not in item.get('optionalDependencies',{}):raise ValueError('Unresolved required npm dependency: '+name)
                    break
                parent=parent.rsplit('/node_modules/',1)[0] if '/node_modules/' in parent else ''
    files=[]
    actual={}
    for number,path in enumerate([*artifacts,*payloads]):
        path=runtime.ordinary(path);actual[path]=runtime.pin(path)
        value={'SPDXID':identifier('artifact:'+str(number)+':'+path.name),'fileName':'./'+('artifact/' if number<len(artifacts) else 'payload/')+path.name,
               'checksums':[{'algorithm':'SHA256','checksumValue':actual[path]['sha256']}],
               'licenseConcluded':'NOASSERTION','licenseInfoInFiles':['NOASSERTION'],'copyrightText':'NOASSERTION'}
        files.append(value)
        relationships.append({'spdxElementId':'SPDXRef-DOCUMENT','relationshipType':'DESCRIBES','relatedSpdxElement':value['SPDXID']})
    report={'spdxVersion':'SPDX-2.3','dataLicense':'CC0-1.0','SPDXID':'SPDXRef-DOCUMENT',
            'name':lock['name']+'-release-sbom','documentNamespace':'https://localai-cowork.invalid/spdx/'+str(uuid.uuid4()),
            'creationInfo':{'created':datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ'),
                            'creators':['Tool: LocalAI-Cowork-release-sbom-2']},
            'packages':packages,'files':files,'relationships':relationships,
            'comment':'Complete locked source graphs, exact selected release bytes and actual inspected native/tool/runtime inventories. Source graphs include build/dev/other-target packages and do not assert binary linkage. External OS imports are explicit requirements. Opaque upstream static dependencies require their original attached SBOM. No licence/security/runtime approval is implied.'}
    coverage={'format':2,'locks':[{**record,'filename':path.relative_to(root).as_posix()} for path,record in lock_pins.items()],
              'payloads':[{'filename':path.name,**record} for path,record in actual.items()],'runtimes':[],'upstream_documents':[]}
    for number,(path,record) in enumerate(actual.items()):
        imports=runtime.native_imports(path)
        if imports:
            inventory={'format':runtime.FORMAT,'name':path.name,'version':None,'files':{path.name:{**record,'native':imports}},
                       'components':[],'namespace_stable':True,'executed_payload':False}
            coverage['runtimes'].append(add_runtime(report,inventory,'native/'+str(number)))
    for number,inventory in enumerate(runtime_inventories):
        if isinstance(inventory,(str,Path)):inventory=strict_json(runtime.ordinary(inventory).read_bytes())
        coverage['runtimes'].append(add_runtime(report,inventory,'runtime/'+str(number)))
    for path,artifact_sha256 in upstream_documents:
        original=runtime.pin(path);upstream=strict_json(Path(path).read_bytes())
        if upstream.get('spdxVersion')!='SPDX-2.3' or not upstream.get('documentNamespace') or artifact_sha256 not in {v['sha256'] for v in actual.values()}:
            raise ValueError('Upstream SPDX document must bind exact selected artifact bytes')
        bound={checksum.get('checksumValue') for item in upstream.get('files',[]) for checksum in item.get('checksums',[]) if checksum.get('algorithm')=='SHA256'}
        if artifact_sha256 not in bound:raise ValueError('Original upstream SPDX has no matching delivered file hash')
        known={'SPDXRef-DOCUMENT'}
        for item in [*upstream.get('packages',[]),*upstream.get('files',[])]:
            sid=item.get('SPDXID')
            if not isinstance(sid,str) or not re.fullmatch(r'SPDXRef-[A-Za-z0-9.+-]+',sid) or sid in known:raise ValueError('Invalid upstream SPDX identity')
            known.add(sid)
        for item in upstream.get('relationships',[]):
            if item.get('spdxElementId') not in known or item.get('relatedSpdxElement') not in known:raise ValueError('Unresolved upstream SPDX relationship')
        external='DocumentRef-'+original['sha256'][:32]
        report.setdefault('externalDocumentRefs',[]).append({'externalDocumentId':external,'spdxDocument':upstream['documentNamespace'],
                                                           'checksum':{'algorithm':'SHA256','checksumValue':original['sha256']}})
        file=next(value for value in files if value['checksums'][0]['checksumValue']==artifact_sha256)
        relationships.append({'spdxElementId':file['SPDXID'],'relationshipType':'OTHER','relatedSpdxElement':external+':SPDXRef-DOCUMENT',
                              'comment':'Original upstream component graph attached to these exact selected artifact bytes'})
        if runtime.pin(path)!=original:raise ValueError('Upstream SPDX changed while reading')
        coverage['upstream_documents'].append({'filename':Path(path).name,**original,'artifact_sha256':artifact_sha256})
    if any(runtime.pin(path)!=record for path,record in {**lock_pins,**actual}.items()):raise ValueError('Source lock or release bytes changed during SBOM generation')
    # Shared OS imports and source graph edges are deduplicated by their exact IDs.
    for key in ('packages','files'):
        unique={}
        for item in report[key]:
            sid=item['SPDXID']
            if sid in unique and unique[sid]!=item:raise ValueError('Conflicting SPDX component')
            unique[sid]=item
        report[key]=sorted(unique.values(),key=lambda item:item['SPDXID'])
    report['relationships']=list({json.dumps(item,sort_keys=True):item for item in report['relationships']}.values())
    if report.get('externalDocumentRefs'):
        report['externalDocumentRefs']=list({item['externalDocumentId']:item for item in report['externalDocumentRefs']}.values())
    report['annotations']=[{'annotationDate':report['creationInfo']['created'],'annotationType':'OTHER',
                           'annotator':'Tool: LocalAI-Cowork-release-sbom-2','comment':json.dumps(coverage,sort_keys=True,separators=(',',':'))}]
    return report


def generate_runtime(inventories, *, external_programs=()):
    """Installed-tool inventory, independent of a build workspace/lock cache."""
    created=datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
    report={'spdxVersion':'SPDX-2.3','dataLicense':'CC0-1.0','SPDXID':'SPDXRef-DOCUMENT',
            'name':'LocalAI-Cowork-installed-tool-runtime','documentNamespace':'https://localai-cowork.invalid/spdx/'+str(uuid.uuid4()),
            'creationInfo':{'created':created,'creators':['Tool: LocalAI-Cowork-release-sbom-2']},
            'packages':[],'files':[],'relationships':[],
            'comment':'Actual selected private software trees and selected external executable bytes. External OS imports and tools are environment requirements; the OS installation is not searched. No configuration, credentials, browser profiles or user files are included.'}
    coverage=[]
    for number,inventory in enumerate(inventories):coverage.append(add_runtime(report,inventory,'runtime/'+str(number)))
    for number,(name,path,version) in enumerate(external_programs):
        original=runtime.pin(path);imports=runtime.native_imports(path)
        record=dict(original)
        if imports:record['native']=imports
        inventory={'format':runtime.FORMAT,'name':name,'version':version,'files':{Path(path).name:record},
                   'components':[],'namespace_stable':True,'executed_payload':False}
        coverage.append(add_runtime(report,inventory,'external-program/'+str(number)))
        if runtime.pin(path)!=original:raise ValueError('External executable changed during inventory')
    for key in ('packages','files'):
        values={}
        for item in report[key]:
            if item['SPDXID'] in values and values[item['SPDXID']]!=item:raise ValueError('Conflicting runtime SPDX identity')
            values[item['SPDXID']]=item
        report[key]=sorted(values.values(),key=lambda item:item['SPDXID'])
    report['relationships']=list({json.dumps(item,sort_keys=True):item for item in report['relationships']}.values())
    report['annotations']=[{'annotationDate':created,'annotationType':'OTHER','annotator':'Tool: LocalAI-Cowork-release-sbom-2',
                           'comment':json.dumps({'format':2,'runtimes':coverage},sort_keys=True,separators=(',',':'))}]
    return report


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--root',type=Path,required=True)
    parser.add_argument('--artifact',type=Path,action='append',default=[]);parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--payload',type=Path,action='append',default=[])
    parser.add_argument('--runtime-inventory',type=Path,action='append',default=[])
    parser.add_argument('--no-desktop',action='store_true');parser.add_argument('--no-npm',action='store_true')
    parser.add_argument('--upstream-sbom',type=Path,action='append',default=[])
    parser.add_argument('--upstream-artifact-sha256',action='append',default=[])
    args=parser.parse_args()
    if not args.output.is_absolute() or args.output.exists():raise ValueError('Select a NEW absolute SBOM output')
    if len(args.upstream_sbom)!=len(args.upstream_artifact_sha256):raise ValueError('Each upstream SBOM needs its exact selected artifact SHA256')
    report=generate(args.root.resolve(strict=True),args.artifact,payloads=args.payload,runtime_inventories=args.runtime_inventory,
                    include_desktop=not args.no_desktop,include_npm=not args.no_npm,
                    upstream_documents=list(zip(args.upstream_sbom,args.upstream_artifact_sha256)))
    with args.output.open('x',encoding='utf-8') as output:json.dump(report,output,indent=2,sort_keys=True)
    print(json.dumps({'sbom':str(args.output),'packages':len(report['packages']),'artifacts':len(report['files'])}))


if __name__=='__main__':main()
