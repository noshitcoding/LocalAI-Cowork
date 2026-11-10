"""Local V2 setup and concrete consent GUI. No listener, remote consent or shell."""
import argparse
import datetime
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import tkinter as tk
from tkinter import filedialog, messagebox, ttk
import uuid

import configure
import windows_sandbox


class DeviceWindow:
    def __init__(self, window, config=None):
        self.window = window
        self.config = None
        self.binary = None
        self.process = None
        self.pending = {}
        window.title('LocalAI Cowork V2 · Device agent')
        window.geometry('920x720')
        tabs = ttk.Notebook(window)
        tabs.pack(fill='both', expand=True)
        setup_tab=ttk.Frame(tabs);canvas=tk.Canvas(setup_tab,highlightthickness=0);scroll=ttk.Scrollbar(setup_tab,orient='vertical',command=canvas.yview)
        canvas.configure(yscrollcommand=scroll.set);scroll.pack(side='right',fill='y');canvas.pack(side='left',fill='both',expand=True)
        setup=ttk.Frame(canvas,padding=12);panel=canvas.create_window((0,0),window=setup,anchor='nw')
        setup.bind('<Configure>',lambda _:canvas.configure(scrollregion=canvas.bbox('all')))
        canvas.bind('<Configure>',lambda e:canvas.itemconfigure(panel,width=e.width))
        consent = ttk.Frame(tabs, padding=12)
        tabs.add(setup_tab, text='Einrichten / Setup')
        tabs.add(consent, text='Lokale Freigaben / Local consent')
        sandbox = ttk.Frame(tabs, padding=12)
        tabs.add(sandbox, text='Windows-Testcomputer / Windows test VM')
        ttk.Label(sandbox, text='Separates Wegwerf-Windows: eigene Gerätekopplung, explizite Ordnerfreigaben. Host-Apps werden nicht übernommen; Schließen verwirft Gastdateien. / Separate disposable Windows: own device pairing and explicit folder grants. Host apps are unavailable; closing discards guest files.', wraplength=850).pack(fill='x', pady=6)
        self.vm_fields = {}
        for name, label, kind in [('profile', 'Eigenes exportiertes VM-Geräteprofil / Separate exported VM profile', 'file'),
                                   ('output', 'Neuer privater VM-Startordner / New private VM launch directory', 'new')]:
            row = ttk.Frame(sandbox); row.pack(fill='x', pady=4)
            ttk.Label(row, text=label).pack(anchor='w')
            value = tk.StringVar(); self.vm_fields[name] = value
            ttk.Entry(row, textvariable=value).pack(side='left', fill='x', expand=True)
            ttk.Button(row, text='Wählen / Choose', command=lambda v=value,k=kind:self.select(v,k)).pack(side='right')
        ttk.Label(sandbox, text='Nur lesbare Hostordner, ein absoluter Pfad pro Zeile / Read-only host folders, one absolute path per line').pack(anchor='w')
        self.vm_read_only = tk.Text(sandbox, height=3); self.vm_read_only.pack(fill='x')
        ttk.Label(sandbox, text='Beschreibbare Hostordner (Änderungen bleiben erhalten) / Writable host folders (changes persist)').pack(anchor='w')
        self.vm_read_write = tk.Text(sandbox, height=3); self.vm_read_write.pack(fill='x')
        self.vm_network = tk.BooleanVar(value=False); self.vm_tools = tk.BooleanVar(value=False)
        ttk.Checkbutton(sandbox, text='Gastnetzwerk erlauben (auch lokale Netze; für Serververbindung nötig) / Enable guest networking (including local networks; required for server connection)', variable=self.vm_network).pack(anchor='w')
        ttk.Checkbutton(sandbox, text='PowerShell und gekoppelte Desktopfähigkeit im Gast erlauben / Permit PowerShell and paired desktop capability in the guest', variable=self.vm_tools).pack(anchor='w')
        ttk.Label(sandbox, text='Verwendet die oben gewählte Windows-Agentdatei und optionale HTTPS-Modellkonfiguration. Das geöffnete Hostprofil bleibt unverändert. Konkrete Aktionen werden im Freigabefenster innerhalb des Gasts bestätigt. / Uses the selected Windows agent executable and optional HTTPS model configuration above. The open host profile is unchanged. Confirm concrete actions in the approval console inside the guest.', wraplength=850).pack(fill='x', pady=6)
        vm_controls = ttk.Frame(sandbox); vm_controls.pack(fill='x')
        ttk.Button(vm_controls, text='Verfügbarkeit prüfen / Check availability', command=self.discover_vm).pack(side='left')
        ttk.Button(vm_controls, text='Startdateien vorbereiten / Prepare launch files', command=lambda:self.prepare_vm(False)).pack(side='left')
        ttk.Button(vm_controls, text='Vorbereiten und VM starten / Prepare and launch VM', command=lambda:self.prepare_vm(True)).pack(side='left')
        self.vm_status = tk.StringVar(value='Noch nicht geprüft / Not checked')
        ttk.Label(sandbox, textvariable=self.vm_status, wraplength=850).pack(fill='x', pady=8)
        self.fields = {}
        for index, (name, label, filekind) in enumerate([
            ('profile', 'Exportiertes Geräteprofil / Exported device profile', 'file'),
            ('output', 'Neuer privater Profilordner / New private profile directory', 'new'),
            ('model_url', 'Eigene Modell-Basis-URL / Own model base URL', None),
            ('model_name', 'Modellname / Model name (optional for direct Office)', None),
            ('model_api','Modell-API: openai / ollama','choice'),
            ('model_implementation','Implementierung: openai_compatible / ollama / vllm','choice'),
            ('model_key_file', 'Eigene Modellschlüssel-Datei / Own model key file (optional)', 'file'),
            ('github_key_file', 'Eigene GitHub-Schlüsseldatei / Own GitHub key file (optional)', 'file'),
            ('mcp_bindings_file', 'Eigene MCP-Bindingsdatei / Own MCP binding file (optional)', 'file'),
            ('agent_binary', 'V2-Agentbinary / V2 agent executable', 'file'),
            ('web_search_url','Eigener HTTPS-Suchendpunkt mit {query} / Own search endpoint',None),
            ('crewai_version','CrewAI: genaue installierte Version / Exact installed version',None),
            ('crewai_source_sha256','CrewAI: ursprünglicher Laufzeit-SHA256 / Original runtime digest',None),
        ]):
            ttk.Label(setup, text=label).grid(row=index*2, column=0, columnspan=2, sticky='w')
            value = tk.StringVar()
            self.fields[name] = value
            if filekind=='choice':
                choices=['openai','ollama'] if name=='model_api' else ['openai_compatible','ollama','vllm'];value.set(choices[0])
                ttk.Combobox(setup,textvariable=value,values=choices,state='readonly').grid(row=index*2+1,column=0,sticky='ew')
            else:ttk.Entry(setup, textvariable=value).grid(row=index*2+1, column=0, sticky='ew')
            if filekind and filekind!='choice':
                ttk.Button(setup, text='Auswählen / Select', command=lambda v=value,k=filekind:self.select(v,k)).grid(row=index*2+1, column=1)
        try:
            discovered=configure.discover_agent(None)
            if discovered:self.fields['agent_binary'].set(str(discovered))
        except (OSError, configure.ConfigurationError):
            pass
        setup.columnconfigure(0, weight=1)
        self.native = tk.BooleanVar(value=False)
        ttk.Checkbutton(setup, text='Ausgewählte native Werkzeuge mit meinen OS-Rechten erlauben / Allow selected native tools with my OS rights', variable=self.native).grid(row=26,column=0,columnspan=2,sticky='w')
        ttk.Label(setup,text='Programme: ein alias=absoluterPfad pro Zeile / Programs: one alias=absolutePath per line').grid(row=27,column=0,columnspan=2,sticky='w')
        self.programs = tk.Text(setup,height=3)
        self.programs.grid(row=28,column=0,columnspan=2,sticky='ew')
        ttk.Button(setup,text='Privates Profil importieren / Import private profile',command=self.import_profile).grid(row=34,column=0,sticky='w',pady=8)
        ttk.Label(setup,text='Erlaubte private HTTPS-Hosts, einer pro Zeile / Private HTTPS hosts, one per line').grid(row=29,column=0,columnspan=2,sticky='w')
        self.allowed_hosts=tk.Text(setup,height=2);self.allowed_hosts.grid(row=30,column=0,columnspan=2,sticky='ew')
        ttk.Label(setup,text='Originale Crew-Profilreferenz=eigener Modellname, eine pro Zeile / Crew profile reference=own model').grid(row=31,column=0,columnspan=2,sticky='w')
        self.crew_profiles=tk.Text(setup,height=2);self.crew_profiles.grid(row=32,column=0,columnspan=2,sticky='ew')
        ttk.Button(setup,text='Installierte CrewAI-Quellen lesen / Inspect installed CrewAI sources',command=self.probe_crew).grid(row=33,column=0,sticky='w',pady=4)
        ttk.Label(setup,text='Office: python + word/excel/powerpoint/powershell; OOXML: eigene Python-Pakete; LibreOffice: libreoffice; sichtbarer Browser: chromium + Playwright. Kein Download/Start erfolgt beim Import.',wraplength=850).grid(row=35,column=0,columnspan=2,sticky='w')
        controls = ttk.Frame(consent)
        controls.pack(fill='x')
        ttk.Button(controls,text='agent.json öffnen / Open agent.json',command=self.open_config).pack(side='left')
        ttk.Button(controls,text='Agent starten / Start agent',command=self.start).pack(side='left')
        ttk.Button(controls,text='Agent geordnet stoppen / Stop agent gracefully',command=self.stop).pack(side='left')
        self.status = tk.StringVar(value='Kein Agentprofil geöffnet / No agent profile open')
        self.capabilities = tk.StringVar(value='')
        ttk.Label(consent,textvariable=self.status,wraplength=850).pack(fill='x',pady=8)
        ttk.Label(consent,textvariable=self.capabilities,wraplength=850).pack(fill='x',pady=4)
        self.requests = tk.Listbox(consent,height=8,exportselection=False)
        self.requests.pack(fill='x')
        self.requests.bind('<<ListboxSelect>>',self.show_request)
        self.details = tk.Text(consent,height=16,wrap='word',state='disabled')
        self.details.pack(fill='both',expand=True,pady=8)
        decisions = ttk.Frame(consent)
        decisions.pack(fill='x')
        ttk.Button(decisions,text='Genau diese Aktion freigeben / Approve this exact action',command=lambda:self.decide('approve')).pack(side='left')
        ttk.Button(decisions,text='Ablehnen / Reject',command=lambda:self.decide('reject')).pack(side='left')
        ttk.Label(consent,text='Freigaben bleiben lokal, laufen ab und gelten nur für den gezeigten Antrag. Das Serverteam kann hier keine Entscheidung schreiben.',wraplength=850).pack(fill='x',pady=8)
        self.tabs = tabs
        if config:
            self.load_config(pathlib.Path(config))
            tabs.select(consent)
        window.protocol('WM_DELETE_WINDOW',self.close)
        window.after(500,self.poll)

    def select(self, value, kind):
        selected = filedialog.asksaveasfilename(title='New private directory name') if kind=='new' else filedialog.askopenfilename()
        if selected:
            value.set(str(pathlib.Path(selected).absolute()))

    def discover_vm(self):
        value = windows_sandbox.discovery()
        self.vm_status.set(json.dumps(value, ensure_ascii=False))

    def prepare_vm(self, start):
        try:
            configure.require(self.config is not None, 'Open the existing host profile first. The guest must use a separate pairing.')
            args = argparse.Namespace(profile=self.vm_fields['profile'].get().strip(), output=self.vm_fields['output'].get().strip(),
                host_executor_id=self.config[1]['registration']['executor_id'], agent_binary=self.fields['agent_binary'].get().strip() or str(self.binary),
                read_only=[line.strip() for line in self.vm_read_only.get('1.0','end').splitlines() if line.strip()],
                read_write=[line.strip() for line in self.vm_read_write.get('1.0','end').splitlines() if line.strip()],
                network=self.vm_network.get(), allow_guest_tools=self.vm_tools.get(), memory_mb=4096,
                model_url=self.fields['model_url'].get().strip() or None, model_name=self.fields['model_name'].get().strip() or None,
                model_key_file=self.fields['model_key_file'].get().strip() or None)
            output = windows_sandbox.prepare(args)
            if start: windows_sandbox.launch(output)
            self.vm_status.set(('VM gestartet / VM started: ' if start else 'Vorbereitet, noch nicht gestartet / Prepared, not started: ')+str(output))
        except (OSError,ValueError,KeyError,configure.ConfigurationError) as error:
            messagebox.showerror('Windows-Testcomputer / Windows test computer', str(error) if isinstance(error,configure.ConfigurationError) else 'Local VM preparation could not be verified')

    def options(self):
        return {'creationflags':subprocess.CREATE_NO_WINDOW} if os.name=='nt' else {}

    def import_profile(self):
        args=[]
        for name,value in self.fields.items():
            if value.get().strip():args.extend(['--'+name.replace('_','-'),value.get().strip()])
        if self.native.get():args.append('--allow-native-tools')
        for program in self.programs.get('1.0','end').splitlines():
            if program.strip():args.extend(['--program',program.strip()])
        for host in self.allowed_hosts.get('1.0','end').splitlines():
            if host.strip():args.extend(['--allowed-host',host.strip()])
        for binding in self.crew_profiles.get('1.0','end').splitlines():
            if binding.strip():args.extend(['--crew-profile-binding',binding.strip()])
        try:
            result=subprocess.run([sys.executable,str(pathlib.Path(configure.__file__).absolute()),*args],capture_output=True,text=True,timeout=30,**self.options())
            if result.returncode:raise configure.ConfigurationError(result.stderr.strip() or 'Profile import refused')
            self.load_config(pathlib.Path(self.fields['output'].get())/'agent.json')
            self.tabs.select(1)
            messagebox.showinfo('Profil importiert / Profile imported','Privates Profil eingerichtet. Agentstart erfolgt erst mit „Agent starten“. / Private profile imported. Click Start agent to connect.')
        except (OSError,ValueError,subprocess.SubprocessError,configure.ConfigurationError) as error:
            messagebox.showerror('Einrichtung fehlgeschlagen / Setup failed',str(error) if isinstance(error,configure.ConfigurationError) else 'Lokale Einrichtung konnte nicht bestätigt werden / Local setup could not be confirmed')

    def probe_crew(self):
        try:
            selected=configure.programs([line.strip() for line in self.programs.get('1.0','end').splitlines() if line.strip()])
            python=selected.get('python')
            if not python:raise configure.ConfigurationError('Select your own python program first / Zuerst eigenes Python auswählen')
            base=pathlib.Path(__file__).resolve().parent;probe=base/'crew-runtime-probe.py'
            if not probe.exists():probe=base.parent.parent/'services/device-agent/src/device_crew_bridge.py'
            configure.selected_path(str(probe))
            result=subprocess.run([python,'-I',str(probe),'--probe'],stdin=subprocess.DEVNULL,capture_output=True,text=True,timeout=30,**self.options())
            if result.returncode or len(result.stdout.encode())>65536:raise configure.ConfigurationError('Installed CrewAI metadata could not be verified / Installierte CrewAI-Metadaten nicht bestätigt')
            runtime=json.loads(result.stdout)['runtime'];self.fields['crewai_version'].set(runtime['version']);self.fields['crewai_source_sha256'].set(runtime['source_sha256'])
            messagebox.showinfo('CrewAI','Quelleninventar gelesen; keine Crew/Modell-/Werkzeugausführung. Gilt nur für diese installierte Distribution. / Source inventory read without Crew/model/tool execution. Applies to this installed distribution only.')
        except (OSError,KeyError,ValueError,subprocess.SubprocessError,configure.ConfigurationError) as error:
            messagebox.showerror('CrewAI',str(error) if isinstance(error,configure.ConfigurationError) else 'Original runtime metadata unavailable')

    def open_config(self):
        selected=filedialog.askopenfilename(filetypes=[('Agent configuration','*.json')])
        if selected:self.load_config(pathlib.Path(selected))

    def load_config(self,path):
        try:
            path=configure.selected_path(str(path.absolute()))
            value=configure.read_json(path,256*1024)
            metadata=configure.read_json(path.parent/'configuration.json')
            binary=configure.selected_path(metadata['agent_binary'])
            result=subprocess.run([str(binary),'--config',str(path),'--check'],capture_output=True,text=True,timeout=30,**self.options())
            if result.returncode:raise configure.ConfigurationError('Agent refused this private configuration; check capabilities and local desktop availability')
            features=subprocess.run([str(binary),'--features'],capture_output=True,text=True,timeout=10,**self.options())
            if features.returncode or not json.loads(features.stdout).get('local_gui_control'):raise configure.ConfigurationError('Use the current V2 agent executable with local GUI control')
            self.config=(path,value)
            self.binary=binary
            self.status.set(str(value['registration']['display_name'])+' · '+value['server_url'])
        except (OSError,KeyError,ValueError,subprocess.SubprocessError,configure.ConfigurationError):
            messagebox.showerror('Profil abgewiesen / Profile refused','Wähle ein privates, gültiges V2-Profil mit passender ausführbarer Agentdatei / Select a valid private V2 profile with its matching executable')

    def start(self):
        if not self.config:return
        if self.process and self.process.poll() is None:return
        path,value=self.config
        try:
            log=path.parent/('agent-log-'+uuid.uuid4().hex+'.txt')
            configure.create_file(log,'')
            with log.open('ab') as output:
                self.process=subprocess.Popen([str(self.binary),'--config',str(path)],stdin=subprocess.DEVNULL,stdout=output,stderr=output,**self.options())
            self.status.set('Agent verbindet sich / Agent connecting …')
        except OSError:
            messagebox.showerror('Start fehlgeschlagen / Start failed','Der eigene Agent konnte nicht gestartet werden / Could not start the owned agent')

    def stop(self):
        if not self.config:return False
        try:
            root=pathlib.Path(self.config[1]['workspace_root'])
            current=configure.read_json(root/'agent-status.json')
            instance=str(uuid.UUID(current['instance_id']))
            if self.process and self.process.poll() is None and current.get('pid')!=self.process.pid:
                raise configure.ConfigurationError('Agent instance does not match the owned process')
            if current['state']=='stopped':return True
            target=root/('.stop-'+instance)
            if not target.exists():configure.create_file(target,instance+'\n')
            self.status.set('Geordneter Stopp angefordert / Graceful stop requested')
            return True
        except (OSError,KeyError,ValueError,configure.ConfigurationError):
            messagebox.showerror('Stopp nicht bestätigt / Stop unconfirmed','Kein gültiger eigener Agentstatus vorhanden / No valid owned agent status available')
            return False

    def poll(self):
        try:
            if self.config:
                root=pathlib.Path(self.config[1]['workspace_root'])
                current=configure.read_json(root/'agent-status.json') if (root/'agent-status.json').exists() else None
                if current:
                    updated=datetime.datetime.fromisoformat(current['updated_at'])
                    now=datetime.datetime.now(datetime.timezone.utc)
                    age=(now-updated).total_seconds()
                    stale=age<0 or age>45
                    heartbeat=current.get('last_heartbeat_at')
                    heartbeat_age=(now-datetime.datetime.fromisoformat(heartbeat)).total_seconds() if heartbeat else None
                    phase=current['state']
                    if phase in {'online','running'} and (heartbeat_age is None or not 0<=heartbeat_age<=45):
                        phase='offline'
                    labels={'online':'Verbunden / Connected','running':'Lauf aktiv / Run active','connecting':'Verbinden / Connecting','offline':'Kein aktueller Heartbeat / No recent heartbeat','network_error':'Server nicht erreichbar / Server request failed','stopped':'Beendet / Stopped'}
                    label='Status veraltet / Status stale' if stale and phase!='stopped' else labels.get(phase,phase)
                    self.status.set(label+' · '+str(current.get('active_run_id') or self.config[1]['registration']['display_name'])+(' · '+str(current['network_error']) if current.get('network_error') else '')+(' · Heartbeat '+str(int(heartbeat_age))+'s' if heartbeat_age is not None and heartbeat_age>=0 else ''))
                availability=root/'agent-availability.json'
                if availability.exists():
                    value=configure.read_json(availability,256*1024)
                    local=value.get('local_execution',{})
                    rights=local.get('user_rights',{}).get('configured',False)
                    self.capabilities.set('Werkzeuge / Tools: '+', '.join(value.get('tools',[]))+' · MCP: '+', '.join(value.get('mcp_bindings',[]))+' · max. '+str(value.get('max_concurrent_runs',1))+' · Benutzerrechte / User rights: '+str(rights)+' · Native Ordnerisolation / Native folder isolation: false')
                if self.process and self.process.poll() is not None:self.status.set('Eigener Agent beendet / Owned agent exited · '+str(self.process.returncode)+(' · '+str(current.get('network_error')) if current and current.get('network_error') else ''))
                pending={}
                directory=root/'local-consent'
                if directory.exists():
                    configure.ordinary(directory,True)
                    for path in sorted(directory.glob('*.request.json'))[:1000]:
                        request=configure.read_json(path,256*1024)
                        identifier=str(uuid.UUID(request['request_id']))
                        expires=datetime.datetime.fromisoformat(request['expires_at'])
                        if path.name!=identifier+'.request.json' or expires<=datetime.datetime.now(datetime.timezone.utc) or (directory/(identifier+'.decision.json')).exists():continue
                        pending[identifier]=request
                if pending!=self.pending:
                    self.pending=pending
                    self.requests.delete(0,'end')
                    for identifier,request in pending.items():self.requests.insert('end',identifier+' · '+str(request['action'].get('tool','desktop/session') if isinstance(request['action'],dict) else 'action'))
                    self.details.configure(state='normal');self.details.delete('1.0','end');self.details.configure(state='disabled')
        except (OSError,KeyError,ValueError,configure.ConfigurationError):
            self.status.set('Privater Gerätestatus nicht lesbar / Private device status unavailable')
        self.window.after(750,self.poll)

    def selected(self):
        selection=self.requests.curselection()
        if not selection:return None
        return list(self.pending)[selection[0]]

    def show_request(self,event=None):
        identifier=self.selected()
        self.details.configure(state='normal');self.details.delete('1.0','end')
        if identifier:
            request=self.pending[identifier];rendered={'request':request}
            original=(request.get('action') or {}).get('original_action')
            if original:
                try:
                    expected=identifier+'.original.json'
                    if original['local_file']!=expected or not 0<original['bytes']<=4*1024*1024:raise ValueError()
                    path=pathlib.Path(self.config[1]['workspace_root'])/'local-consent'/expected;configure.selected_path(str(path));raw=path.read_bytes()
                    if len(raw)!=original['bytes'] or hashlib.sha256(raw).hexdigest()!=original['sha256']:raise ValueError()
                    rendered['complete_original_action']=json.loads(raw)
                except (KeyError,OSError,ValueError,configure.ConfigurationError):
                    rendered['original_error']='Original bytes unavailable or changed; approval is blocked'
            self.details.insert('1.0',json.dumps(rendered,indent=2,ensure_ascii=False))
        self.details.configure(state='disabled')

    def decide(self,decision):
        identifier=self.selected()
        if not identifier or not self.config:return
        try:
            original=(self.pending[identifier].get('action') or {}).get('original_action')
            if original and decision=='approve':
                expected=identifier+'.original.json'
                if original['local_file']!=expected or not 0<original['bytes']<=4*1024*1024:raise ValueError()
                path=pathlib.Path(self.config[1]['workspace_root'])/'local-consent'/expected;configure.selected_path(str(path));raw=path.read_bytes()
                if len(raw)!=original['bytes'] or hashlib.sha256(raw).hexdigest()!=original['sha256']:raise ValueError()
            result=subprocess.run([str(self.binary),'consent','--data',self.config[1]['workspace_root'],'--request',identifier,'--decision',decision],capture_output=True,text=True,timeout=10,**self.options())
            if result.returncode:raise ValueError()
        except (OSError,ValueError,subprocess.SubprocessError):
            messagebox.showerror('Freigabe abgewiesen / Consent refused','Antrag wurde bereits entschieden, ist abgelaufen oder hat sich verändert / Request already decided, expired or changed')

    def close(self):
        if getattr(self,'closing',False):return
        if self.process and self.process.poll() is None:
            if not messagebox.askyesno('Agent stoppen / Stop agent','Eigenen Agent vor dem Schließen geordnet stoppen? / Stop the owned agent before closing?'):return
            if not self.stop():return
            self.closing=True
            def finish(remaining=75):
                if self.process.poll() is not None:self.window.destroy()
                elif remaining:self.window.after(200,lambda:finish(remaining-1))
                else:
                    self.closing=False
                    self.status.set('Stopp noch nicht bestätigt / Stop not yet confirmed')
                    messagebox.showerror('Stopp nicht bestätigt / Stop unconfirmed','Der Agent beendet laufende Aktionen noch. Das Fenster bleibt zur Kontrolle offen; Stopp kann erneut angefordert werden. / The agent has not confirmed shutdown. The window remains open; retry the stop request.')
            self.window.after(200,finish)
        else:self.window.destroy()


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config',type=pathlib.Path)
    args=parser.parse_args()
    root=tk.Tk()
    DeviceWindow(root,args.config)
    root.mainloop()
