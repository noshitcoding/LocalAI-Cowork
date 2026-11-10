import type {BrowserEngine,BrowserMode} from '@cowork/client';
export function browserSetupRequired(programs:readonly string[]|undefined,engine:BrowserEngine,mode:BrowserMode):boolean {
  return mode==='managed'&&(!programs?.includes('python')||!programs.includes(engine));
}
export function browserSetupMessage(reason:string,language:'de'|'en'):string|undefined {
  const messages:Record<string,[string,string]>={
    browser_install_runtime_first:['Richte Python, Playwright und den gewählten Browser unter „Werkzeuge einrichten“ ein. Lade danach die Werkzeugkonfiguration neu.','Set up Python, Playwright and the selected browser under “Configure tool runtimes”, then reload the tool configuration.'],
    browser_profile_busy:['Das Projektprofil wird bereits verwendet. Schließe die andere Sitzung, bevor du es erneut öffnest.','The project profile is in use. Close the other session before opening it again.'],
    browser_select_extension:['Verbinde die Erweiterung mit dieser App und gib mindestens einen Tab frei. Wähle danach die Verbindung.','Connect the extension to this app and share at least one tab, then select the connection.'],
    browser_connection_busy:['Diese Erweiterungsverbindung gehört bereits zu einer Sitzung. Wähle die offene Sitzung oder schließe sie zuerst.','This extension connection already belongs to a session. Select the open session or close it first.'],
    browser_session_limit:['Schließe eine offene Browsersitzung, bevor du eine weitere öffnest.','Close an open browser session before opening another.'],
  };
  const key=Object.keys(messages).find(key=>reason.includes(key));return key?messages[key][language==='de'?0:1]:undefined;
}
