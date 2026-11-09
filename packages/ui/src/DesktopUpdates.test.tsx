import {afterEach,describe,expect,it,vi} from 'vitest';
import {renderToStaticMarkup} from 'react-dom/server';
import {DesktopUpdates} from './DesktopUpdates';
import {version} from '../../../package.json';
describe('web update status',()=>{
  afterEach(()=>vi.unstubAllGlobals());
  it('explains the operator update path and identifies the actual web version',()=>{
    vi.stubGlobal('window',{});
    const markup=renderToStaticMarkup(<DesktopUpdates/>);
    expect(markup).toContain(version);
    expect(markup).toMatch(/Betreiber|operator/);
    expect(markup).not.toContain('<button');
  });
});
