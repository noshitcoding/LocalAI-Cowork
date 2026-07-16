const fs = require('fs');

const file = 'app/src/components/SettingsView.tsx';
let content = fs.readFileSync(file, 'utf8');

// Container
content = content.replace(/className="settings-view"/g, 'className="max-w-4xl mx-auto p-4 sm:p-6 space-y-8"');
content = content.replace(/<h1>Einstellungen<\/h1>/g, '<h1 className="text-3xl font-bold tracking-tight mb-6 mt-2">Einstellungen</h1>');

// Section / Panel
content = content.replace(/<section className="panel">/g, '<section className="bg-card text-card-foreground rounded-lg border shadow-sm p-5 sm:p-6 space-y-5">');

// Panel Headers
content = content.replace(/<h2>/g, '<h2 className="text-xl font-semibold border-b pb-3 mb-2">');
content = content.replace(/<div className="panel-heading-row">/g, '<div className="flex items-center justify-between border-b pb-3 mb-2">');
content = content.replace(/<h2 className="text-xl font-semibold border-b pb-3 mb-2">Diagnose-Log<\/h2>/g, '<h2 className="text-xl font-semibold">Diagnose-Log</h2>');

// Grids
content = content.replace(/<div className="grid">/g, '<div className="grid gap-5 sm:grid-cols-2 lg:grid-cols-3">');

// Labels
content = content.replace(/<label>/g, '<label className="text-sm font-medium leading-none flex flex-col gap-2 cursor-pointer">');

// Inputs
content = content.replace(/<input\s+type="checkbox"/g, '<input className="h-4 w-4 shrink-0 rounded-sm border border-primary ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50" type="checkbox"');
content = content.replace(/<input\s+type="range"/g, '<input className="my-2 w-full accent-primary" type="range"');
// Regular inputs that don't have className already
content = content.replace(/<input(?!\s+className|\s+type="checkbox"|\s+type="range")/g, '<input className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors file:border-0 file:bg-transparent file:text-sm file:font-medium placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"');

// Select
content = content.replace(/<select/g, '<select className="flex h-9 w-full items-center justify-between rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-sm ring-offset-background placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-50"');

// Textarea
content = content.replace(/<textarea/g, '<textarea className="flex min-h-[80px] w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"');

// Actions
content = content.replace(/<div className="actions">/g, '<div className="flex flex-wrap items-center gap-3 pt-3">');

// Buttons
content = content.replace(/className="btn-secondary"/g, 'className="inline-flex items-center justify-center rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 border border-input bg-background shadow-sm hover:bg-accent hover:text-accent-foreground h-9 px-4 py-2"');
// All other buttons that do NOT have a className (no trailing closing brace since we just match `<button`)
content = content.replace(/<button(?!\s+type="button"\s+className="|\s+className=")/g, '<button className="inline-flex items-center justify-center rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 bg-primary text-primary-foreground shadow hover:bg-primary/90 h-9 px-4 py-2"');

// Wait! Some buttons have disabled={XYZ} onClick={...} and no className. The negative lookahead above ensures we only target buttons doing neither.
// It's probably easier to just replace `<button ` with `<button className="..." ` unless it already has a className or is the dialog close button.
// Actually, earlier the bug was mostly from `onChange={(e) => ... }` in inputs.

// Error & Cards
content = content.replace(/className="error"/g, 'className="text-destructive font-medium text-sm p-3 bg-destructive/10 rounded-md border border-destructive/20"');
content = content.replace(/className="card"/g, 'className="flex flex-col gap-2 rounded-md border bg-muted/30 p-4 text-sm"');
content = content.replace(/className="tool-result"/g, 'className="bg-muted p-3 mt-2 rounded-md font-mono text-xs overflow-x-auto border whitespace-pre-wrap max-h-60"');
content = content.replace(/className="text-destructive font-medium text-sm p-3 bg-destructive\/10 rounded-md border border-destructive\/20">\{error\}<\/p>/g, 'className="text-destructive font-medium text-sm p-3 bg-destructive/10 rounded-md border border-destructive/20 my-4">{error}</p>');

// Tool Lists
content = content.replace(/className="tool-list"/g, 'className="grid gap-2"');
content = content.replace(/<li key=\{path\} className="tool-item">/g, '<li key={path} className="flex items-center justify-between p-3 rounded-md border bg-card hover:bg-accent/50 transition-colors">');
content = content.replace(/className="tool-item tool-item-action-row"/g, 'className="flex items-center justify-between p-3 rounded-md border bg-card hover:bg-accent/50 transition-colors"');
content = content.replace(/className="tool-item"/g, 'className="flex items-center justify-between p-3 rounded-md border bg-card hover:bg-accent/50 transition-colors"');
content = content.replace(/<label key=\{key\} className="flex items-center justify-between p-3 rounded-md border bg-card hover:bg-accent\/50 transition-colors">/g, '<label key={key} className="flex items-center justify-between p-3 rounded-md border bg-card hover:bg-accent/50 transition-colors cursor-pointer group">');

// Logs
content = content.replace(/className="panel-empty"/g, 'className="text-sm text-muted-foreground italic py-4 text-center border border-dashed rounded-md"');
content = content.replace(/className="log-list"/g, 'className="flex flex-col gap-3"');
content = content.replace(/className=\{`log-entry log-\$\{entry.level\}`\}/g, 'className={`flex flex-col gap-1.5 p-3 rounded-md border text-sm ${entry.level === "error" ? "bg-destructive/10 border-destructive/20" : entry.level === "warn" ? "bg-yellow-500/10 border-yellow-500/20" : "bg-muted/40"}`}');
content = content.replace(/className="log-entry log-info"/g, 'className="flex flex-col gap-1.5 p-3 rounded-md border text-sm bg-muted/40"');
content = content.replace(/className="log-entry-header"/g, 'className="flex items-center gap-2 text-xs font-medium text-muted-foreground mb-1"');
content = content.replace(/className="log-level"/g, 'className="uppercase tracking-wider font-bold"');
content = content.replace(/className="log-time"/g, 'className="ml-auto opacity-70"');

// Fix checkbox span styling to prevent overlapping text
content = content.replace(/<span>\{label\}<\/span>/g, '<span className="text-sm cursor-pointer select-none group-hover:text-accent-foreground">{label}</span>');

fs.writeFileSync(file, content);
console.log('Settings View refactored cleanly!');