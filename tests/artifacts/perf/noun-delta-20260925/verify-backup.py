import datetime,json,pathlib,sqlite3,time,os
root=pathlib.Path('/opt/nockcloud/backups/pma-storage-efficiency-20260925T1635Z')
inputs=pathlib.Path('/backup-inputs')
def emit(stage,**details):
 print(json.dumps(dict(time=datetime.datetime.now(datetime.timezone.utc).isoformat(),stage=stage,**details)),flush=True)
start=time.monotonic();db=inputs/'event-log.sqlite3';before=db.stat()
wal=inputs/'event-log.sqlite3-wal'
assert not wal.exists() or wal.stat().st_size==0,'nonempty WAL needs separate consistent snapshot'
rows=json.loads((root/'copy-inventory.json').read_text())
for row in rows:
 p=inputs/row['path'];assert p.is_file() and p.stat().st_size==row['bytes'],row['path']
emit('inventory_verified',files=len(rows),sqlite_bytes=before.st_size)
conn=sqlite3.connect(f'file:{db}?mode=ro&immutable=1',uri=True)
conn.execute('PRAGMA query_only=ON');conn.execute('PRAGMA cache_size=-131072');conn.execute('PRAGMA temp_store=MEMORY')
emit('sqlite_integrity_started',sqlite_version=sqlite3.sqlite_version)
result=[r[0] for r in conn.execute('PRAGMA integrity_check')];conn.close()
after=db.stat();assert (before.st_size,before.st_mtime_ns)==(after.st_size,after.st_mtime_ns),'input changed'
report=dict(result=result,ok=result==['ok'],seconds=time.monotonic()-start,sqlite_bytes=after.st_size)
path=root/'backup-integrity-result.json';tmp=path.with_suffix('.tmp');tmp.write_text(json.dumps(report,indent=2)+'\n');os.replace(tmp,path)
emit('complete',**report)
assert report['ok'],result
