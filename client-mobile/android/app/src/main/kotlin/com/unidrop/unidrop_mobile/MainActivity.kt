package com.unidrop.unidrop_mobile

import android.os.Build
import android.provider.Settings
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        // 上报对端 roster 的真实设备名：device_info_plus 的 model 只是机型
        // （如 "Mi 10"），用户要的是「设置→关于手机」里的设备名称。
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "unidrop/device_name")
            .setMethodCallHandler { call, result ->
                if (call.method == "getDeviceName") {
                    val name = Settings.Global.getString(contentResolver, "device_name")
                        ?: Settings.Secure.getString(contentResolver, "bluetooth_name")
                        ?: Build.MODEL
                    result.success(name)
                } else {
                    result.notImplemented()
                }
            }
    }
}
