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
import MethodChannel, { MethodCallHandler, MethodResult } from '@ohos/flutter_ohos/src/main/ets/plugin/common/MethodChannel';
import MethodCall from '@ohos/flutter_ohos/src/main/ets/plugin/common/MethodCall';
import Want from '@ohos.app.ability.Want';
import AbilityConstant from '@ohos.app.ability.AbilityConstant';
import settings from '@ohos.settings';
import deviceInfo from '@ohos.deviceInfo';
import fs from '@ohos.file.fs';
import abilityAccessCtrl, { Permissions } from '@ohos.abilityAccessCtrl';
import connection from '@ohos.bluetooth.connection';
import window from '@ohos.window';

export default class EntryAbility extends FlutterAbility {
  configureFlutterEngine(flutterEngine: FlutterEngine) {
    super.configureFlutterEngine(flutterEngine)
    GeneratedPluginRegistrant.registerWith(flutterEngine)
    this.registerDeviceNameChannel(flutterEngine)
  }

  // 上报对端 roster 的真实设备名。取数优先级：
  // 1. 「设置→关于本机」设备名称（settings.general.DEVICE_NAME）——
  //    READ_LOCAL_DEVICE_NAME 需 AGC profile 放行（provisionEnable），
  //    profile 未放行时只回出厂默认名（机型）；
  // 2. 蓝牙本机名（connection.getLocalName，user_grant 的
  //    ACCESS_BLUETOOTH 即可，零售机可装可授），华为设备蓝牙名默认
  //    跟随设备名称；
  // 3. 市场机型名（marketName，如「Pura 70 Ultra」）。
  private registerDeviceNameChannel(engine: FlutterEngine) {
    const channel = new MethodChannel(
      engine.dartExecutor.getBinaryMessenger(), 'unidrop/device_name');
    const handler: MethodCallHandler = {
      onMethodCall: (call: MethodCall, result: MethodResult) => {
        if (call.method !== 'getDeviceName') {
          result.notImplemented();
          return;
        }
        const btName = this.bluetoothLocalName();
        try {
          settings.getValue(this.context, settings.general.DEVICE_NAME)
            .then((v) => {
              console.log('UniClip deviceName sources: settings=' + v + ' bt=' + btName);
              result.success(this.pickDeviceName(v, btName));
            })
            .catch(() => {
              console.log('UniClip deviceName sources: settings=<err> bt=' + btName);
              result.success(this.pickDeviceName('', btName));
            });
        } catch (e) {
          result.success(this.pickDeviceName('', btName));
        }
      }
    };
    channel.setMethodCallHandler(handler);
  }

  private pickDeviceName(settingsName: string, btName: string): string {
    // settings 未放行时回出厂名（字面量未必等于 marketName，无法可靠比对），
    // 而华为设备的蓝牙名始终跟随用户自设设备名，故蓝牙名优先。
    if (btName.length > 0) {
      return btName;
    }
    if (settingsName.length > 0) {
      return settingsName;
    }
    return deviceInfo.marketName;
  }

  private bluetoothLocalName(): string {
    try {
      return connection.getLocalName();
    } catch (e) {
      console.error('UniClip bt getLocalName failed: ' + JSON.stringify(e));
      return '';
    }
  }

  async onCreate(want: Want, launchParam: AbilityConstant.LaunchParam) {
    this.handleSetupUri(want);
    await super.onCreate(want, launchParam);
  }

  onWindowStageCreate(windowStage: window.WindowStage): void {
    super.onWindowStageCreate(windowStage);
    this.requestDeviceNamePermission();
  }

  // ACCESS_BLUETOOTH 是 user_grant 权限：首次启动弹窗申请。
  // 授权后 connection.getLocalName() 才可用。已授权时该调用直接返回，不重复弹窗。
  private requestDeviceNamePermission() {
    try {
      const atManager = abilityAccessCtrl.createAtManager();
      const permissions: Array<Permissions> = ['ohos.permission.ACCESS_BLUETOOTH'];
      atManager.requestPermissionsFromUser(this.context, permissions)
        .then((data) => {
          console.log('UniClip ACCESS_BLUETOOTH authResults: ' + JSON.stringify(data.authResults));
        })
        .catch((e: Error) => {
          console.error('UniClip ACCESS_BLUETOOTH request failed: ' + e.message);
        });
    } catch (e) {
      console.error('UniClip ACCESS_BLUETOOTH setup failed');
    }
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
