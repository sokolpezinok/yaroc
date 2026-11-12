import * as assert from 'node:assert/strict';
import { test } from 'node:test';
import { MqttClient, YarocEvent } from '../index';

test('MqttClient initializes and stops cleanly with valid timezone', async () => {
  const client = new MqttClient([], [], '+02:00');

  const firstEvent = await new Promise<YarocEvent>((resolve, reject) => {
    client.start((err, event) => {
      if (err) {
        reject(err);
        return;
      }
      resolve(event);
    });
  });

  assert.deepEqual(firstEvent, { status: 'initialized' });
  client.stop();
});

test('MqttClient reports an error for an invalid timezone', async () => {
  const client = new MqttClient([], [], 'invalid-tz');

  await assert.rejects(
    new Promise<YarocEvent>((resolve, reject) => {
      client.start((err, event) => {
        if (err) {
          reject(err);
          return;
        }
        resolve(event);
      });
    }),
  );
});
