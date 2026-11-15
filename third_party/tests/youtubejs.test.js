import { Innertube, YTNodes } from '../bundle/cf-worker.js';

describe('embedded YouTube.js', () => {
  test.each([false, true])('session accepts optional install data: %s', async (withInstallData) => {
    const device = Array(108).fill(null);
    device[0] = 'en';
    device[1] = 'US';
    device[13] = 'synthetic-visitor';
    device[16] = '2.20260925.01.00';
    if (withInstallData) device[61] = ['synthetic-install'];
    let requests = 0;
    const youtube = await Innertube.create({
      generate_session_locally: false,
      retrieve_player: false,
      retrieve_innertube_config: false,
      enable_session_cache: false,
      fail_fast: true,
      fetch: async (input) => {
        expect(String(input)).toEqual('https://www.youtube.com/sw.js_data');
        requests++;
        return new Response(")]}'\n" + JSON.stringify([[null, null, [[device], 'synthetic-key']]]));
      }
    });
    expect(requests).toEqual(1);
    expect(youtube.session.context.client.visitorData).toEqual('synthetic-visitor');
    expect(youtube.session.context.client.configInfo?.appInstallData).toEqual(withInstallData ? 'synthetic-install' : undefined);
  });

  test('signed-in playlist preferences parse their text label', () => {
    const field = new YTNodes.ToggleFormField({ label: { simpleText: 'Show unavailable videos' }, toggled: true });
    expect(field.label.toString()).toEqual('Show unavailable videos');
    expect(field.toggled).toEqual(true);
  });
});
