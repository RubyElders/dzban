import AppKit
import DzbanBridge
@testable import DzbanApplication
import SwiftUI
import XCTest

@MainActor
final class NativeResultGridTests: XCTestCase {
    private func table(in view: NSView?) -> NativeResultTable? {
        guard let view else { return nil }
        if let table = view as? NativeResultTable { return table }
        return view.subviews.lazy.compactMap { self.table(in: $0) }.first
    }
    private func until(_ predicate: () -> Bool) async throws {
        let deadline = ContinuousClock.now + .seconds(10)
        while !predicate() {
            if ContinuousClock.now >= deadline { throw NSError(domain: "NativeResultGridTests", code: 1) }
            try await Task.sleep(for: .milliseconds(10))
        }
    }
    private func key(_ table: NativeResultTable, code: UInt16, characters: String, modifiers: NSEvent.ModifierFlags = []) {
        table.keyDown(with: NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: modifiers, timestamp: 0,
            windowNumber: table.window?.windowNumber ?? 0, context: nil, characters: characters, charactersIgnoringModifiers: characters, isARepeat: false, keyCode: code)!)
    }

    func testSelectionKeepsReadableTextInLightAndDarkAppearance() async throws {
        let store = ApplicationStore(bridge: try Bridge.fixture())
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1100, height: 760), styleMask: [.titled, .resizable], backing: .buffered, defer: false)
        let host = NSHostingView(rootView: RootView(store: store))
        host.wantsLayer = true
        host.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        window.contentView = host
        window.makeKeyAndOrderFront(nil)
        defer { window.contentView = NSView(); window.orderOut(nil) }
        await store.start()
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        let document = try XCTUnwrap(store.document)
        store.editor(for: document).restore(text: "few")
        await store.performEditorIntent(.submit)
        try await until { self.table(in: window.contentView)?.numberOfRows == 3 && store.results.value(row: 0, column: 0) != nil }
        let table = try XCTUnwrap(table(in: window.contentView))
        for (name, appearance) in [("light", NSAppearance.Name.aqua), ("dark", NSAppearance.Name.darkAqua)] {
            window.appearance = NSAppearance(named: appearance)
            for mode in ["cells", "rows", "columns", "all"] {
                switch mode {
                case "cells": table.gridChanged?("cell", ["row": 0, "column": 2, "extend": false])
                case "rows": table.gridChanged?("row", ["row": 0, "gesture": "replace"])
                case "columns": table.gridChanged?("column", ["column": 2, "gesture": "replace"])
                default: table.selectAll(nil)
                }
                window.makeFirstResponder(table)
                try await Task.sleep(for: .milliseconds(30))
                let row = try XCTUnwrap(table.rowView(atRow: 0, makeIfNecessary: true))
                row.isEmphasized = true
                window.contentView?.layoutSubtreeIfNeeded()
                XCTAssertEqual(row.interiorBackgroundStyle, .normal, "\(name) \(mode)")
                for column in 0..<2 {
                    let cell = try XCTUnwrap(table.view(atColumn: column, row: 0, makeIfNecessary: true) as? NSTableCellView)
                    let text = try XCTUnwrap(cell.textField)
                    let bitmap = try XCTUnwrap(text.bitmapImageRepForCachingDisplay(in: text.bounds))
                    text.effectiveAppearance.performAsCurrentDrawingAppearance { text.cacheDisplay(in: text.bounds, to: bitmap) }
                    var foreground = 0
                    for y in 0..<bitmap.pixelsHigh {
                        for x in 0..<bitmap.pixelsWide {
                            guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB), color.alphaComponent > 0.8 else { continue }
                            let brightness = (color.redComponent + color.greenComponent + color.blueComponent) / 3
                            if name == "light" ? brightness < 0.5 : brightness > 0.5 { foreground += 1 }
                        }
                    }
                    XCTAssertGreaterThan(foreground, 3, "\(name) \(mode) column \(column) has no contrasting text")
                }
                if let directory = ProcessInfo.processInfo.environment["DZBAN_GRID_SCREENSHOTS"], let bitmap = host.bitmapImageRepForCachingDisplay(in: host.bounds) {
                    host.cacheDisplay(in: host.bounds, to: bitmap)
                    try FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
                    try bitmap.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: directory).appendingPathComponent("selection-\(name)-\(mode).png"))
                }
            }
        }
        try await store.shutdown()
    }

    func testSharedGridKeepsRangesHeadersAndFrozenColumnOrder() throws {
        let columns = try JSONDecoder().decode([Column].self, from: Data(#"[{"name":"id","typeName":"int"},{"name":"value","typeName":"text"}]"#.utf8))
        let grid = try ResultGrid(columns: columns, rows: 8)
        try grid.apply("cell", ["row": 2, "column": 1, "extend": false])
        try grid.apply("move", ["horizontal": 1, "vertical": 2, "extend": true])
        XCTAssertTrue(grid.state.contains(row: 3, column: 1))
        XCTAssertTrue(grid.state.contains(row: 4, column: 2))
        XCTAssertFalse(grid.state.contains(row: 1, column: 1))
        try grid.apply("column", ["column": 2, "gesture": "replace"])
        try grid.apply("resize", ["rows": 12])
        XCTAssertEqual(grid.state.ranges.first?.end, 12)
        XCTAssertEqual(grid.state.columns, [2])
        try grid.apply("freeze", ["column": 2])
        let layout = try grid.apply("layout", ["widths": [40, 100, 100], "scroll": 0, "viewport": 150, "column": 2])
        XCTAssertEqual(layout.right, [2])
        XCTAssertEqual(layout.reveal, 0)
        XCTAssertEqual(layout.frozen, [0, 2])
    }

    func testNativeKeysCopyRawCellsAndProjectedRanges() async throws {
        let store = ApplicationStore(bridge: try Bridge.fixture())
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1100, height: 760), styleMask: [.titled, .resizable], backing: .buffered, defer: false)
        let host = NSHostingView(rootView: RootView(store: store))
        host.wantsLayer = true
        host.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        window.contentView = host
        window.makeKeyAndOrderFront(nil)
        defer { window.contentView = NSView(); window.orderOut(nil) }
        await store.start()
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        let document = try XCTUnwrap(store.document)
        store.editor(for: document).restore(text: "few")
        await store.performEditorIntent(.submit)
        try await until { self.table(in: window.contentView)?.grid != nil && self.table(in: window.contentView)?.numberOfRows == 3 }
        let table = try XCTUnwrap(table(in: window.contentView))
        store.results.gridAction?("rows")
        XCTAssertEqual(table.grid?.state.mode, "rows")
        XCTAssertEqual(table.grid?.state.cursor?.row, 0)
        store.results.gridAction?("rows")
        XCTAssertEqual(table.grid?.state.mode, "cells")
        table.gridChanged?("cell", ["row": 0, "column": 2, "extend": false])
        let pasteboard = NSPasteboard(name: NSPasteboard.Name(UUID().uuidString))
        defer { pasteboard.releaseGlobally() }
        store.results.copyGrid(to: pasteboard)
        try await until { pasteboard.string(forType: .string) == "žluť\nNULL😀" }
        XCTAssertFalse(pasteboard.types?.contains(NSPasteboard.PasteboardType("net.daringfireball.markdown")) == true)
        key(table, code: 125, characters: "", modifiers: .shift)
        XCTAssertEqual(table.grid?.state.cursor, ResultPosition(row: 1, column: 2))
        let scroll = try XCTUnwrap(table.enclosingScrollView)
        XCTAssertEqual(scroll.contentView.bounds.minX, -(scroll.verticalRulerView?.ruleThickness ?? 0), accuracy: 1)
        store.results.copyGrid(to: pasteboard)
        try await until { pasteboard.string(forType: .string) == "| value |\n| --- |\n| žluť<br>NULL😀 |\n| NULL |\n" }
        key(table, code: 49, characters: " ", modifiers: .control)
        XCTAssertEqual(table.grid?.state.mode, "rows")
        XCTAssertEqual(table.grid?.state.columns, [0, 1, 2])
        key(table, code: 49, characters: " ", modifiers: .control)
        XCTAssertEqual(table.grid?.state.mode, "cells")
        let header = try XCTUnwrap(table.headerView)
        func clickHeader(_ column: Int, modifiers: NSEvent.ModifierFlags) {
            let rect = header.headerRect(ofColumn: column)
            let point = header.convert(NSPoint(x: rect.midX, y: rect.midY), to: nil)
            header.mouseDown(with: NSEvent.mouseEvent(with: .leftMouseDown, location: point, modifierFlags: modifiers,
                timestamp: 0, windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!)
        }
        clickHeader(0, modifiers: [])
        clickHeader(1, modifiers: .command)
        XCTAssertEqual(table.grid?.state.mode, "columns")
        XCTAssertEqual(table.grid?.state.columns, [1, 2])
        let accessibleHeader = try XCTUnwrap(header.accessibilityChildren()?.first as? RowNumberElement)
        store.editor(for: document).requestFocus()
        XCTAssertTrue(accessibleHeader.accessibilityPerformPress())
        XCTAssertTrue(window.firstResponder === table)
        XCTAssertEqual(table.grid?.state.columns, [1])
        table.selectNumberedRow(0)
        table.selectNumberedRow(2, modifiers: .command)
        XCTAssertEqual(table.grid?.state.ranges.map(\.start), [0, 2])
        store.results.select(row: 2, column: 1)
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(table.grid?.state.mode, "rows")
        XCTAssertEqual(table.grid?.state.ranges.map(\.start), [0, 2])
        table.peekRow?()
        try await until { store.rowInspector.address != nil }
        XCTAssertEqual(store.rowInspector.address?.row, 2)
        XCTAssertEqual(store.rowInspector.address?.column, 0)
        table.resetPeek()
        store.rowInspector.close()
        table.selectAll(nil)
        table.reloadData()
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(table.grid?.state.mode, "rows")
        store.results.copyGrid(to: pasteboard)
        try await until { pasteboard.string(forType: .string)?.hasPrefix("| # | id | value |\n") == true }
        store.editor(for: document).restore(text: "wide")
        await store.performEditorIntent(.submit)
        try await until { table.numberOfColumns == 128 }
        store.editor(for: document).restore(text: "few")
        await store.performEditorIntent(.submit)
        try await until { table.numberOfColumns == 2 && store.results.grid?.state.cursor == nil }
        store.editor(for: document).requestFocus()
        let scroll = try XCTUnwrap(table.enclosingScrollView)
        let emptyPoint = scroll.convert(NSPoint(x: scroll.bounds.midX, y: scroll.bounds.maxY - 20), to: nil)
        scroll.mouseDown(with: NSEvent.mouseEvent(with: .leftMouseDown, location: emptyPoint, modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!)
        XCTAssertTrue(window.firstResponder === table)
        table.peekRow?()
        try await until { store.rowInspector.address != nil }
        XCTAssertEqual(store.rowInspector.address?.row, 0)
        XCTAssertEqual(store.rowInspector.address?.column, 0)
        table.resetPeek()
        store.rowInspector.close()
        try await store.shutdown()
    }

    func testFrozenColumnsRenderAtBothEdgesAndSurviveMatchingRerun() async throws {
        let store = ApplicationStore(bridge: try Bridge.fixture())
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1100, height: 760), styleMask: [.titled, .resizable], backing: .buffered, defer: false)
        let host = NSHostingView(rootView: RootView(store: store))
        host.wantsLayer = true
        host.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        window.contentView = host
        window.makeKeyAndOrderFront(nil)
        defer { window.contentView = NSView(); window.orderOut(nil) }
        await store.start()
        let connected = await store.connect(profile: "fixture")
        XCTAssertTrue(connected)
        let document = try XCTUnwrap(store.document)
        store.editor(for: document).restore(text: "wide")
        await store.performEditorIntent(.submit)
        try await until { self.table(in: window.contentView)?.grid?.schema.count == 128 && self.table(in: window.contentView)?.numberOfRows == 500 }
        let table = try XCTUnwrap(table(in: window.contentView))
        let scroll = try XCTUnwrap(table.enclosingScrollView as? ResultScrollView)
        let clip = scroll.contentView
        let initialOrigin = clip.bounds.origin
        clip.scroll(to: NSPoint(x: table.bounds.width - clip.bounds.width, y: 0))
        scroll.reflectScrolledClipView(clip)
        try await until { store.results.value(row: 0, column: 127) != nil }
        try await Task.sleep(for: .milliseconds(100))
        window.contentView?.layoutSubtreeIfNeeded()
        let visibleColumns = table.columnIndexes(in: table.dataVisibleRect)
        XCTAssertEqual(table.gridVisibleColumns, visibleColumns)
        let firstVisibleColumn = try XCTUnwrap(visibleColumns.first)
        table.selectNumberedRow(499)
        let horizontal = clip.bounds.minX
        table.peekRow?()
        try await until { store.rowInspector.address != nil }
        XCTAssertEqual(store.rowInspector.address?.row, 499)
        XCTAssertEqual(store.rowInspector.address?.column, firstVisibleColumn)
        XCTAssertEqual(clip.bounds.minX, horizontal, accuracy: 1)
        table.resetPeek()
        store.rowInspector.close()
        clip.scroll(to: initialOrigin)
        scroll.reflectScrolledClipView(clip)
        table.gridChanged?("cell", ["row": 0, "column": 1, "extend": false])
        table.gridChanged?("freeze", ["column": 128])
        try await until { store.results.value(row: 0, column: 127) != nil }
        XCTAssertNotNil(scroll.frozenAnchor(row: 0, column: 127))
        table.gridChanged?("freeze", ["column": 1])
        clip.scroll(to: NSPoint(x: table.bounds.width - clip.bounds.width, y: 0))
        scroll.reflectScrolledClipView(clip)
        XCTAssertNotNil(scroll.frozenAnchor(row: 0, column: 0))
        XCTAssertTrue(table.gridLayout()?.left.contains(1) == true)
        table.gridChanged?("cell", ["row": 0, "column": 1, "extend": false])
        let origin = clip.bounds.origin
        key(table, code: 49, characters: " ")
        try await until { store.rowInspector.address != nil }
        XCTAssertEqual(store.rowInspector.address?.column, 0)
        XCTAssertEqual(clip.bounds.minX, origin.x, accuracy: 1)
        table.resetPeek()
        store.rowInspector.close()
        table.gridChanged?("cell", ["row": 0, "column": 125, "extend": false])
        table.gridChanged?("move", ["horizontal": 1, "vertical": 2, "extend": true])
        let generation = store.document?.generation ?? 0
        await store.performEditorIntent(.submit)
        try await until { (store.document?.generation ?? 0) > generation && store.document?.execution == .finished && self.table(in: window.contentView)?.grid?.schema.count == 128 }
        XCTAssertEqual(store.results.grid?.state.frozen, [0, 1, 128])
        XCTAssertEqual(store.results.grid?.state.columns, [125, 126])
        XCTAssertEqual(store.results.grid?.state.ranges.first?.start, 0)
        XCTAssertEqual(store.results.grid?.state.ranges.first?.end, 3)
        try await until { store.results.value(row: 0, column: 127) != nil && store.results.value(row: 0, column: 0) != nil }
        XCTAssertLessThanOrEqual(store.results.peakInFlight, 2)
        XCTAssertLessThanOrEqual(store.results.cacheBytes, 8 * 1024 * 1024)
        if let directory = ProcessInfo.processInfo.environment["DZBAN_GRID_SCREENSHOTS"], let view = window.contentView,
           let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
            try await Task.sleep(for: .milliseconds(100))
            view.layoutSubtreeIfNeeded()
            view.cacheDisplay(in: view.bounds, to: bitmap)
            try FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
            try bitmap.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: directory).appendingPathComponent("frozen-columns.png"))
        }
        let originalPage = try XCTUnwrap(store.selectedPage)
        await store.newDocument()
        try await until { store.results.scope == nil }
        await store.selectPage(originalPage)
        try await until { store.results.scope?.document == document.document && store.results.grid?.schema.count == 128 }
        XCTAssertEqual(store.results.grid?.state.frozen, [0, 1, 128])
        XCTAssertEqual(store.results.grid?.state.columns, [125, 126])
        XCTAssertEqual(store.results.grid?.state.ranges.first?.end, 3)
        store.editor(for: document).restore(text: "few")
        await store.performEditorIntent(.submit)
        try await until { self.table(in: window.contentView)?.grid?.schema.count == 2 }
        XCTAssertEqual(store.results.grid?.state.frozen, [0])
        try await store.shutdown()
    }
}
