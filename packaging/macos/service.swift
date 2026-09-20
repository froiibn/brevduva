// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0
//
// brevduva-service — Brevduva.app 안의 서비스 정의를 SMAppService로 등록·해제·조회한다
// (2026-09-21, 시험용). macOS 13 이상.
//
//   Brevduva.app/Contents/MacOS/brevduva-service [register | unregister | status]
//
// SMAppService는 호출한 프로세스의 "메인 묶음" 안에서 plist를 찾는다 — 그래서 이 도구는
// 앱 묶음의 Contents/MacOS/ 안에 있어야 한다. 제품 구현에서는 brv가 직접 호출하는 것으로
// 바꿀 수 있다; 시험에서는 표시·동작 실측이 목적이라 가장 짧은 길(Swift 몇 줄)을 쓴다.

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
