// 直连 Dart VM service 的排障 CLI（鸿蒙 fork attach 不可用时的替代通道）——
// 工具的输出就是它的职责，print 是有意为之。
// ignore_for_file: avoid_print
import 'dart:async';
import 'dart:convert';
import 'dart:io';

// 直连 Dart VM service 的排障客户端（鸿蒙 attach 因 fork 的 listViews 为空
// 不可用时的替代通道）。
//
// 用法：
//   vm_stdout <ws-uri>          订阅 Stdout/Logging 流并转储
//   vm_stdout <ws-uri> --probe  额外探测：evaluate print 回环 + 主 isolate getStack
//   vm_stdout <ws-uri> --eval <libUriPattern> <expr>
//                               在 uri 含 pattern 的库作用域内 evaluate 表达式
//                               （库未加载时退化到 rootLib）
Future<void> main(List<String> args) async {
  final uri = args[0];
  final probe = args.contains('--probe');
  final evalIdx = args.indexOf('--eval');
  final evalPattern = evalIdx >= 0 ? args[evalIdx + 1] : null;
  final evalExpr = evalIdx >= 0 ? args[evalIdx + 2] : null;
  final ws = await WebSocket.connect(uri);
  var nextId = 0;
  final pending = <String, Completer<Map<String, dynamic>>>{};

  Future<Map<String, dynamic>> rpc(String method, [Map<String, dynamic>? params]) {
    final id = '${++nextId}';
    final c = Completer<Map<String, dynamic>>();
    pending[id] = c;
    ws.add(jsonEncode(
        {'jsonrpc': '2.0', 'id': id, 'method': method, if (params != null) 'params': params}));
    return c.future;
  }

  ws.listen((data) {
    final msg = jsonDecode(data as String) as Map<String, dynamic>;
    if (msg.containsKey('result') || msg.containsKey('error')) {
      final c = pending.remove(msg['id']?.toString());
      c?.complete(msg);
      if (c == null && msg['id'] != null) {
        print('rpc resp ${msg['id']}: ${jsonEncode(msg['result'] ?? msg['error'])}');
      }
      return;
    }
    final event = msg['params']?['event'];
    if (event is Map) {
      final stream = event['streamId'];
      if (stream == 'Stdout' || stream == 'Logging') {
        final bytes = event['bytes'] as String?;
        if (bytes != null) stdout.write(utf8.decode(base64.decode(bytes)));
      }
    }
  });

  await rpc('streamListen', {'streamId': 'Stdout'});
  await rpc('streamListen', {'streamId': 'Logging'});

  if (probe) {
    final vm = await rpc('getVM');
    final isolates = (vm['result']?['isolates'] as List?) ?? [];
    print('== isolates: ${isolates.map((e) => '${e['name']}(${e['id']})').join(', ')}');
    for (final iso in isolates) {
      final isoId = iso['id'] as String;
      final name = iso['name'];
      // print 回环：验证 Stdout 捕获是否生效
      final evalRes = await rpc('evaluate', {
        'isolateId': isoId,
        'targetId': isoId,
        'expression': 'print("vm-probe-$name")',
      });
      print('== evaluate($name): ${jsonEncode(evalRes['result']?['result'] ?? evalRes['error'] ?? evalRes)}');
      // 当前栈：挂起定位
      final stack = await rpc('getStack', {'isolateId': isoId});
      final frames = (stack['result']?['frames'] as List?) ?? [];
      print('== getStack($name): ${frames.length} frames');
      for (final f in frames.take(15)) {
        final fn = f['function'] as Map<String, dynamic>?;
        final loc = f['location'] as Map<String, dynamic>?;
        print('   ${fn?['name']}  ${loc?['script']?['uri']}:${loc?['line']}');
      }
      final asyncFrames = (stack['result']?['asyncCausalFrames'] as List?) ?? [];
      if (asyncFrames.isNotEmpty) {
        print('== asyncCausal($name): ${asyncFrames.length} frames');
        for (final f in asyncFrames.take(20)) {
          final fn = f['function'] as Map<String, dynamic>?;
          final loc = f['location'] as Map<String, dynamic>?;
          print('   ${fn?['name']}  ${loc?['script']?['uri']}:${loc?['line']}');
        }
      }
    }
  }

  if (evalExpr != null) {
    final vm = await rpc('getVM');
    final isolates = (vm['result']?['isolates'] as List?) ?? [];
    if (isolates.isEmpty) {
      print('!! 无 isolate');
      exit(2);
    }
    final isoId = isolates.first['id'] as String;
    final iso = await rpc('getIsolate', {'isolateId': isoId});
    final libraries = (iso['result']?['libraries'] as List?) ?? [];
    String? targetId;
    for (final lib in libraries) {
      final libUri = lib['uri'] as String? ?? '';
      if (evalPattern != null && libUri.contains(evalPattern)) {
        targetId = lib['id'] as String;
        break;
      }
    }
    targetId ??= iso['result']?['rootLib']?['id'] as String?;
    print('== eval target: $targetId (pattern=$evalPattern)');
    final res = await rpc('evaluate', {
      'isolateId': isoId,
      'targetId': targetId,
      'expression': evalExpr,
    });
    print('== eval result: ${jsonEncode(res['result']?['result'] ?? res['error'] ?? res)}');
    await stdout.flush();
    // 给 then/catchError 回调留执行窗口
    await Future<void>.delayed(const Duration(seconds: 5));
    await stdout.flush();
    exit(0);
  }

  await ws.done;
}
