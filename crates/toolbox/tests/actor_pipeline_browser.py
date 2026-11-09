"""Actual local HTTP evidence and the production private guest text formatter.

The isolated guest image is separately pinned and needs its configured Chromium
runtime. This test does not claim to start runsc or grant loopback to an Actor.
"""
import http.server,importlib.util,json,pathlib,threading,unittest,urllib.request
spec=importlib.util.spec_from_file_location('private_browser',pathlib.Path(__file__).resolve().parents[3]/'deploy/v2/android-private-browser.py');browser=importlib.util.module_from_spec(spec);spec.loader.exec_module(browser)
class PipelineTests(unittest.TestCase):
 def test_real_local_http_bytes_become_bounded_cited_fetch_output(self):
  body='Actual HTTP evidence 🧠'.encode()
  class Handler(http.server.BaseHTTPRequestHandler):
   def log_message(self,*args):pass
   def do_GET(self):self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
  server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
  try:
   url='http://127.0.0.1:'+str(server.server_port)+'/evidence'
   with urllib.request.urlopen(url,timeout=2) as response:text,truncated=browser.pipeline_text('fetch_text',url,response.status,'Fixture',response.read().decode())
   self.assertIn('Status: 200\nTitle: Fixture',text);self.assertIn('Actual HTTP evidence 🧠',text);self.assertFalse(truncated)
  finally:server.shutdown();server.server_close()
 def test_search_retains_actual_result_urls_and_fails_on_challenge(self):
  items=[{'title':'Evidence','url':'https://duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org%2Fsource','snippet':'Original result'}]
  text,_=browser.pipeline_text('search','https://html.duckduckgo.com/',200,'Search',results=items);self.assertIn('https://example.org/source',text)
  with self.assertRaisesRegex(ValueError,'challenge'):browser.pipeline_text('search','https://html.duckduckgo.com/',200,'Challenge',results=[])
  oversized=[{'title':'🧠'*1000,'url':'https://example.org/'+'x'*8192,'snippet':'🧠'*1000}]*5
  text,truncated=browser.pipeline_text('search','https://html.duckduckgo.com/',200,'Search',results=oversized);self.assertTrue(truncated);self.assertLessEqual(len(text.encode()),48000);self.assertNotIn('\ufffd',text)
if __name__=='__main__':unittest.main()
