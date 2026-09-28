/*
* Copyright (c) 2023 Hunan OpenValley Digital Industry Development Co., Ltd.
* Licensed under the Apache License, Version 2.0 (the "License");
* you may not use this file except in compliance with the License.
* You may obtain a copy of the License at
*
*     http://www.apache.org/licenses/LICENSE-2.0
*
* Unless required by applicable law or agreed to in writing, software
* distributed under the License is distributed on an "AS IS" BASIS,
* WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
* See the License for the specific language governing permissions and
* limitations under the License.
*/

import { FlutterAbility } from '@ohos/flutter_ohos'
import { GeneratedPluginRegistrant } from '../plugins/GeneratedPluginRegistrant';
import FlutterEngine from '@ohos/flutter_ohos/src/main/ets/embedding/engine/FlutterEngine';
import Want from '@ohos.app.ability.Want';
import AbilityConstant from '@ohos.app.ability.AbilityConstant';
import fs from '@ohos.file.fs';

export default class EntryAbility extends FlutterAbility {
  configureFlutterEngine(flutterEngine: FlutterEngine) {
    super.configureFlutterEngine(flutterEngine)
    GeneratedPluginRegistrant.registerWith(flutterEngine)
  }

  async onCreate(want: Want, launchParam: AbilityConstant.LaunchParam) {
    this.handleSetupUri(want);
    await super.onCreate(want, launchParam);
  }

  onNewWant(want: Want, launchParam: AbilityConstant.LaunchParam) {
    this.handleSetupUri(want);
    super.onNewWant(want, launchParam);
  }

  // 调试/CI 注入口：aa start -U "unidropmobile://setup?server=…&account=…&psk=…"。
  // 鸿蒙 platform channel 断流期间键盘不可用，设置经 URI → 落盘
  // bootstrap_config.json → Dart 在 native.start 前读取覆盖。文件一次性消费。
  private handleSetupUri(want: Want) {
    try {
      const uri = want?.uri as string;
      if (!uri || !uri.startsWith('unidropmobile://setup')) {
        return;
      }
      const query = uri.substring(uri.indexOf('?') + 1);
      const params: Record<string, string> = {};
      query.split('&').forEach((kv) => {
        const idx = kv.indexOf('=');
        if (idx > 0) {
          params[kv.substring(0, idx)] = decodeURIComponent(kv.substring(idx + 1));
        }
      });
      const path = this.context.filesDir + '/bootstrap_config.json';
      const file = fs.openSync(path, fs.OpenMode.READ_WRITE | fs.OpenMode.CREATE | fs.OpenMode.TRUNC);
      fs.writeSync(file.fd, JSON.stringify(params));
      fs.closeSync(file);
      console.log('UniClip setup uri written: ' + path);
    } catch (e) {
      console.error('UniClip setup uri failed: ' + e);
    }
  }
}
