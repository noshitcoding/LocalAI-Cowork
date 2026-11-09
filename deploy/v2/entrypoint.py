"""Read explicitly mounted operator secrets, then replace this process with daemon."""
import os,pathlib,stat,sys,urllib.parse

def secret(variable):
    path=pathlib.Path(os.environ[variable]);assert path.is_absolute() and not path.is_symlink()
    with path.open('rb') as stream:
        metadata=os.fstat(stream.fileno());assert stat.S_ISREG(metadata.st_mode) and metadata.st_nlink==1
        raw=stream.read(4097)
    assert 0<len(raw)<=4096 and b'\0' not in raw
    return raw.decode('utf-8').strip()

if 'COWORK_DATABASE_PASSWORD_FILE' in os.environ:
    password=secret('COWORK_DATABASE_PASSWORD_FILE')
    user=urllib.parse.quote(os.environ.get('COWORK_DATABASE_USER','cowork'),safe='')
    host=os.environ.get('COWORK_DATABASE_HOST','postgres');database=os.environ.get('COWORK_DATABASE_NAME','cowork')
    assert host.replace('-','').replace('.','').isalnum() and database.replace('_','').isalnum()
    port=int(os.environ.get('COWORK_DATABASE_PORT','5432'));assert 1<=port<=65535
    sslmode=os.environ.get('COWORK_DATABASE_SSLMODE','prefer');assert sslmode in {'disable','prefer','require','verify-ca','verify-full'}
    query={'sslmode':sslmode}
    if os.environ.get('COWORK_DATABASE_SSLROOTCERT'):query['sslrootcert']=os.environ['COWORK_DATABASE_SSLROOTCERT']
    os.environ['COWORK_DATABASE_URL']='postgres://'+user+':'+urllib.parse.quote(password,safe='')+'@'+host+':'+str(port)+'/'+database+'?'+urllib.parse.urlencode(query)
# S3 *_FILE custody is validated and held directly by the Rust S3 adapter.
# Do not create mutually conflicting direct environment credentials here.
os.umask(0o077)
os.execv('/usr/local/bin/cowork-daemon',['cowork-daemon',*sys.argv[1:]])
