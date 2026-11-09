import {render,screen} from '@testing-library/react'
import {describe,expect,it,vi} from 'vitest'
import type {MessageRecord} from '../runtime/contracts'
import RemoteThreadMessages from './RemoteThreadMessages'
vi.mock('./RemoteRunComposer',()=>({default:()=>null}))
describe('server assistant content',()=>{
  it('renders worker content without exposing its transport envelope as the reply',()=>{
    const message={id:'reply',role:'assistant',created_at:'2026-10-24T00:00:00Z',content:{content:'SERVER_OK',completion_tokens:98,prompt_tokens:230,steps:1}} as MessageRecord
    render(<RemoteThreadMessages client={null} threadId="thread" initialMessages={[message]}/> )
    expect(screen.getByText('SERVER_OK')).toBeInTheDocument()
    expect(screen.queryByText(/completion_tokens/)).not.toBeInTheDocument()
  })
})
