// @testable: los init memberwise de Protocol.swift son internal (ver petición
// de cambio de contrato de WP4: añadir init públicos).
@testable import ActaMacProtocol
import Foundation
import Testing

@Test func `decodes a command with snake_case args`() throws {
    let line = #"{"id":7,"cmd":"capture.retarget","args":{"pids":[1,2],"bundle_ids":["com.microsoft.teams2"]}}"#
    let req = try ProtocolCoding.decoder().decode(Request.self, from: Data(line.utf8))
    #expect(req == Request(id: 7, command: .captureRetarget(.init(pids: [1, 2], bundleIds: ["com.microsoft.teams2"]))))
}

@Test func `decodes a command without args`() throws {
    let req = try ProtocolCoding.decoder().decode(Request.self, from: Data(#"{"id":1,"cmd":"capture.stop"}"#.utf8))
    #expect(req.command == .captureStop)
}

@Test func `unknown command keeps the id for the error reply`() {
    #expect(throws: RequestDecodingError.unknownCommand(id: 9, cmd: "nope")) {
        try ProtocolCoding.decoder().decode(Request.self, from: Data(#"{"id":9,"cmd":"nope"}"#.utf8))
    }
}

@Test func `encodes an event line like the Rust side expects`() throws {
    let frame = AudioFrameEvent(track: .system, seq: 42, ptsUs: 4_200_000, sampleRate: 16_000, channels: 1, format: "s16le", data: "AAA=")
    let data = try ProtocolCoding.encoder().encode(EventLine(ts: 1, event: .audioFrame(frame)))
    let json = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
    #expect(json["type"] as? String == "event")
    #expect(json["event"] as? String == "audio.frame")
    let payload = try #require(json["data"] as? [String: Any])
    #expect(payload["pts_us"] as? Int == 4_200_000)
    #expect(payload["sample_rate"] as? Int == 16_000)
}

@Test func `speaker embedding model id matches the Rust constant`() {
    #expect(speakerEmbeddingModel == "fluidaudio-wespeaker-community1-256")
}

@Test func `decodes the meeting title and calendar commands`() throws {
    let t = try ProtocolCoding.decoder().decode(Request.self, from: Data(#"{"id":3,"cmd":"meeting.titles","args":{"pid":42,"bundle_id":"com.microsoft.teams2"}}"#.utf8))
    #expect(t.command == .meetingTitles(MeetingTitlesArgs(pid: 42, bundleId: "com.microsoft.teams2")))
    let c = try ProtocolCoding.decoder().decode(Request.self, from: Data(#"{"id":4,"cmd":"calendar.events","args":{"from_unix_ms":1,"to_unix_ms":2,"request_access":true}}"#.utf8))
    #expect(c.command == .calendarEvents(CalendarEventsArgs(fromUnixMs: 1, toUnixMs: 2, requestAccess: true)))
}

@Test func `decodes the system profile and installed apps commands`() throws {
    let p = try ProtocolCoding.decoder().decode(Request.self, from: Data(#"{"id":5,"cmd":"system.profile"}"#.utf8))
    #expect(p.command == .systemProfile)
    let a = try ProtocolCoding.decoder().decode(Request.self, from: Data(#"{"id":6,"cmd":"apps.installed","args":{"icons_dir":"/data/cache/app-icons","icon_px":128}}"#.utf8))
    #expect(a.command == .appsInstalled(AppsInstalledArgs(iconsDir: "/data/cache/app-icons", iconPx: 128)))
}

@Test func `encodes the profile and the apps like the Rust side expects`() throws {
    func json(_ v: some Encodable) throws -> [String: Any] {
        try #require(try JSONSerialization.jsonObject(with: ProtocolCoding.encoder().encode(v)) as? [String: Any])
    }
    let profile = try json(SystemProfileResult(fullName: "Ana García", preferredLanguages: ["es-ES", "en-GB"]))
    #expect(profile["full_name"] as? String == "Ana García")
    #expect(profile["preferred_languages"] as? [String] == ["es-ES", "en-GB"])
    #expect(try json(SystemProfileResult(fullName: nil, preferredLanguages: []))["full_name"] == nil)

    let apps = try json(AppsInstalledResult(apps: [
        InstalledAppWire(bundleId: "com.apple.Safari", name: "Safari", path: "/Applications/Safari.app", running: true, iconPath: "/i/com.apple.Safari.png"),
        InstalledAppWire(bundleId: "com.rara", name: "Rara", path: "/Applications/Rara.app", running: false, iconPath: nil),
    ]))
    let list = try #require(apps["apps"] as? [[String: Any]])
    #expect(list[0]["bundle_id"] as? String == "com.apple.Safari")
    #expect(list[0]["icon_path"] as? String == "/i/com.apple.Safari.png")
    #expect(list[0]["running"] as? Bool == true)
    #expect(list[1]["icon_path"] == nil)
}

@Test func `decodes the clipboard commands and encodes the clipboard image event`() throws {
    let s = try ProtocolCoding.decoder().decode(Request.self, from: Data(#"{"id":8,"cmd":"clipboard.start","args":{"out_dir":"/m/captures","max_width":1920}}"#.utf8))
    #expect(s.command == .clipboardStart(ClipboardStartArgs(outDir: "/m/captures", maxWidth: 1920)))
    let t = try ProtocolCoding.decoder().decode(Request.self, from: Data(#"{"id":9,"cmd":"clipboard.stop"}"#.utf8))
    #expect(t.command == .clipboardStop)
    let ev = ScreenFrameEvent(path: "/m/captures/1-manual.jpg", atUnixMs: 1, reason: .manual, phash: "00", width: 8, height: 6)
    let data = try ProtocolCoding.encoder().encode(EventLine(ts: 1, event: .clipboardImage(ev)))
    let json = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
    #expect(json["event"] as? String == "clipboard.image")
    let payload = try #require(json["data"] as? [String: Any])
    #expect(payload["reason"] as? String == "manual")
    #expect(payload["at_unix_ms"] as? Int == 1)
}

@Test func `screen frame carries the optional thumbnail signature`() throws {
    let ev = ScreenFrameEvent(path: "/m/captures/1-slide_change.jpg", atUnixMs: 1, reason: .slideChange, phash: "00", width: 8, height: 6, thumb: "0a0b")
    let data = try ProtocolCoding.encoder().encode(ev)
    let json = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
    #expect(json["thumb"] as? String == "0a0b")
    let old = try ProtocolCoding.decoder().decode(ScreenFrameEvent.self, from: Data(#"{"path":"/p","at_unix_ms":1,"reason":"manual","phash":"00","width":1,"height":1,"keyword":null}"#.utf8))
    #expect(old.thumb == nil, "events from older sidecars still decode")
}
