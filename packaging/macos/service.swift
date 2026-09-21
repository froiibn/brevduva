// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0
//
// brevduva-service — Brevduva.app 안의 서비스 정의를 SMAppService로 등록·해제·조회한다
// (2026-09-21). macOS 13 이상. brv가 이 도구를 실행하고 마지막 `after: <상태>` 줄을 읽는다
// (crates/brv/src/macos_bundle.rs parse_helper_status) — 출력 형식을 바꾸면 그쪽도 같이 바꾼다.
//
//   Brevduva.app/Contents/MacOS/brevduva-service [register | unregister | status]
//
// SMAppService는 호출한 프로세스의 "메인 묶음" 안에서 plist를 찾는다 — 그래서 이 도구는
// 앱 묶음의 Contents/MacOS/ 안에 있어야 한다. brv(Rust)가 직접 부르지 않는 이유: 이 API는
// Swift·Objective-C로만 제공되고, 다리를 놓으면 맥에서만 컴파일되는 코드와 외부 의존성이 는다.

import Foundation
import ServiceManagement

let plistName = "dev.brevduva.brv-daemon.plist"
let service = SMAppService.agent(plistName: plistName)

func name(_ s: SMAppService.Status) -> String {
    switch s {
    case .notRegistered: return "notRegistered"
    case .enabled: return "enabled"
    case .requiresApproval: return "requiresApproval"
    case .notFound: return "notFound"
    @unknown default: return "unknown(\(s.rawValue))"
    }
}

let command = CommandLine.arguments.dropFirst().first ?? "status"
print("bundle: \(Bundle.main.bundlePath)")
print("before: \(name(service.status))")
var failed = false
switch command {
case "register":
    do { try service.register(); print("register: ok") } catch {
        print("register: failed — \(error)")
        failed = true
    }
case "unregister":
    do { try service.unregister(); print("unregister: ok") } catch {
        print("unregister: failed — \(error)")
        failed = true
    }
case "status":
    break
default:
    print("usage: brevduva-service [register | unregister | status]")
    exit(2)
}
print("after: \(name(service.status))")
exit(failed ? 1 : 0)
