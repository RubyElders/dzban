import AppKit
import DzbanBridge
import SwiftUI

struct ResultTable: NSViewRepresentable {
    @ObservedObject var model: ResultViewport
    let inspect: () -> Void
    let cancel: () -> Bool
    let peek: (NSView, NSRect) -> Void
    let releasePeek: (UInt64) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(model: model) }
    func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSScrollView, context: Context) -> CGSize? {
        let width = proposal.width.flatMap { $0.isFinite ? $0 : nil } ?? 600
        let height = proposal.height.flatMap { $0.isFinite ? $0 : nil } ?? 240
        return CGSize(width: width, height: height)
    }
    func makeNSView(context: Context) -> NSScrollView {
        #if DZBAN_TEST_FIXTURES
        ViewUpdateDiagnostics.frames.append("ResultTable.swift")
        defer { ViewUpdateDiagnostics.frames.removeLast() }
        #endif
        let table = NativeResultTable()
        let scroll = ResultScrollView(frame: .zero)
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        scroll.documentView = table
        let numbers = ResultRowNumbers(scrollView: scroll, orientation: .verticalRuler)
        numbers.clientView = table
        scroll.verticalRulerView = numbers
        scroll.hasVerticalRuler = true
        scroll.rulersVisible = true
        table.delegate = context.coordinator
        table.dataSource = context.coordinator
        table.headerView = ResultGridHeaderView()
        table.style = .plain
        table.usesAlternatingRowBackgroundColors = true
        table.columnAutoresizingStyle = .noColumnAutoresizing
        table.allowsColumnReordering = false
        table.allowsMultipleSelection = true
        table.rowHeight = 24
        table.gridStyleMask = [.solidVerticalGridLineMask]
        table.gridColor = .separatorColor
        table.intercellSpacing = .zero
        table.setAccessibilityIdentifier("results.table")
        table.setAccessibilityLabel("Query results")
        table.setAccessibilityHelp("Copy a cell as text or a selection as Markdown. Shift extends a selection. Command Shift R toggles whole rows. X freezes the current column. Space opens the row preview; double-click opens a full cell value.")
        context.coordinator.table = table
        table.userNavigated = { [weak coordinator = context.coordinator] in coordinator?.cancelRestoration() }
        table.didLayout = { [weak coordinator = context.coordinator] in
            guard let coordinator, !coordinator.applying else { return }
            coordinator.sizeColumns()
            coordinator.restorePosition()
        }
        table.target = context.coordinator
        table.doubleAction = #selector(Coordinator.inspectCell)
        scroll.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(context.coordinator, selector: #selector(Coordinator.scrolled), name: NSView.boundsDidChangeNotification, object: scroll.contentView)
        return scroll
    }
    func updateNSView(_ scroll: NSScrollView, context: Context) {
        #if DZBAN_TEST_FIXTURES
        ViewUpdateDiagnostics.frames.append("ResultTable.swift")
        defer { ViewUpdateDiagnostics.frames.removeLast() }
        #endif
        guard let table = scroll.documentView as? NativeResultTable else { return }
        let coordinator = context.coordinator
        coordinator.inspect = inspect
        table.inspectCell = inspect
        table.copyRows = { [weak model] rows in model?.copyRows(rows) }
        table.cancelQuery = cancel
        table.peekRow = { [weak table, weak coordinator] in
            if let table, let coordinator {
                let visible = table.gridVisibleColumns
                let selected = coordinator.model.selected
                let selectedVisible = selected.map { visible.contains($0.column) } ?? false
                if table.rowNumberSelected || !selectedVisible, let visibleColumn = visible.first {
                    let column = selected == nil ? 0 : visibleColumn
                    let visibleRow = table.rows(in: table.visibleRect).location
                    let row = selected?.row ?? (visibleRow == NSNotFound ? 0 : max(0, visibleRow))
                    coordinator.model.select(row: row, column: column)
                }
            }
            if let table, let selected = coordinator?.model.selected {
                table.focusedColumn = selected.column
                if !table.selectedRowIndexes.contains(selected.row) { table.selectRowIndexes(IndexSet(integer: selected.row), byExtendingSelection: false) }
                if let anchor = table.rowPreviewAnchor { peek(anchor.view, anchor.rect) }
            }
        }
        table.releaseRow = releasePeek
        table.selectCell = { [weak coordinator] row, column in
            coordinator?.cancelRestoration()
            coordinator?.model.select(row: row, column: column)
            coordinator?.rememberPosition()
        }
        if coordinator.scope != model.scope {
            coordinator.rememberPosition()
            coordinator.scope = model.scope
            coordinator.layoutPosition = nil
            coordinator.pendingPosition = model.scope.flatMap { model.tablePositions[$0.document] }
            coordinator.sizing = []
            table.rowNumberSelected = false
        }
        table.canPeek = model.availableRows > 0 && !model.columns.isEmpty
        table.usesAlternatingRowBackgroundColors = model.availableRows > 0
        coordinator.applying = true
        if coordinator.columns != model.columns {
            coordinator.columns = model.columns
            table.columnAutoresizingStyle = .noColumnAutoresizing
            for column in table.tableColumns { table.removeTableColumn(column) }
            for (index, column) in model.columns.enumerated() {
                let native = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("result.column.\(index)"))
                let header = ResultColumnHeaderCell(textCell: column.name)
                header.leadingInset = index == 0 ? WorkspaceLayout.gutterInset : WorkspaceLayout.columnInset
                header.trailingInset = index == model.columns.count - 1 ? WorkspaceLayout.textInset : WorkspaceLayout.columnInset
                native.headerCell = header
                native.width = max(60, (column.name as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 12)]).width + 44)
                native.minWidth = 60
                native.maxWidth = .greatestFiniteMagnitude
                table.addTableColumn(native)
            }
        }
        model.prepareGrid()
        coordinator.configureGrid()
        coordinator.sizeColumns()
        let selectedRows = table.selectedRowIndexes.intersection(IndexSet(integersIn: 0..<model.availableRows))
        table.reloadData()
        table.selectRowIndexes(selectedRows, byExtendingSelection: false)
        (scroll.verticalRulerView as? ResultRowNumbers)?.update(rows: model.availableRows)
        if let selected = model.selected, selected.row < model.availableRows {
            if !table.selectedRowIndexes.contains(selected.row) { table.selectRowIndexes(IndexSet(integer: selected.row), byExtendingSelection: false) }
            table.focusedColumn = selected.column
        } else { table.deselectAll(nil) }
        if model.availableRows > 0, let clip = table.enclosingScrollView?.contentView {
            let maximumY = max(0, table.rect(ofRow: model.availableRows - 1).maxY - clip.bounds.height)
            if clip.bounds.minY > maximumY { clip.scroll(to: NSPoint(x: clip.bounds.minX, y: maximumY)) }
        }
        coordinator.restorePosition()
        coordinator.syncGrid()
        coordinator.applying = false
        model.focusCycle?.update(table, included: model.isFocusable) { [weak coordinator] in coordinator?.focus() }
        coordinator.scheduleVisible()
        if coordinator.focusRequest != model.focusRequest {
            coordinator.focusRequest = model.focusRequest
            let request = model.focusRequest
            DispatchQueue.main.async { [weak coordinator, weak table] in
                guard let coordinator, let table, coordinator.model.focusRequest == request,
                      table.window != nil else { return }
                coordinator.focus()
            }
        }
    }

    static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        NotificationCenter.default.removeObserver(coordinator)
        if let table = coordinator.table { coordinator.model.focusCycle?.remove(table) }
        coordinator.table?.didLayout = nil
        coordinator.table?.userNavigated = nil
        coordinator.table?.resetPeek()
        coordinator.table?.delegate = nil
        coordinator.table?.dataSource = nil
    }

    @MainActor
    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate {
        let model: ResultViewport
        weak var table: NativeResultTable?
        var columns: [Column] = []
        var scope: ResultScope?
        var pendingPosition: ResultTablePosition?
        var layoutPosition: ResultTablePosition?
        var sizing: [ResultColumnSizing] = []
        var focusRequest: UInt64 = 0
        var applying = false
        var inspect: (() -> Void)?
        private var scheduled = false
        private var visibleColumns = IndexSet()
        #if DZBAN_TEST_FIXTURES
        private var reportedLayout = false
        #endif
        init(model: ResultViewport) { self.model = model }
        func numberOfRows(in tableView: NSTableView) -> Int { model.availableRows }
        func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
            guard let tableColumn, let column = tableView.tableColumns.firstIndex(of: tableColumn),
                  tableView.columnIndexes(in: tableView.visibleRect).contains(column) else { return nil }
            let identifier = NSUserInterfaceItemIdentifier("result.cell")
            let cell = (tableView.makeView(withIdentifier: identifier, owner: nil) as? ResultCellView) ?? ResultCellView()
            if cell.textField == nil {
                cell.identifier = identifier
                let text = NSTextField(labelWithString: "")
                text.setAccessibilityElement(true)
                text.setAccessibilityRole(.staticText)
                text.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
                text.lineBreakMode = .byTruncatingTail
                text.maximumNumberOfLines = 1
                cell.addSubview(text)
                cell.textField = text
            }
            cell.leadingInset = column == 0 ? WorkspaceLayout.gutterInset : WorkspaceLayout.columnInset
            cell.trailingInset = column == model.columns.count - 1 ? WorkspaceLayout.textInset : WorkspaceLayout.columnInset
            let value = model.value(row: row, column: column)
            let display = value.map(ResultCellDisplay.preview) ?? "..."
            cell.textField?.stringValue = display
            cell.textField?.setAccessibilityIdentifier("result.cell.\(row).\(column)")
            cell.textField?.textColor = value?.text == nil ? .secondaryLabelColor : .labelColor
            cell.setAccessibilityIdentifier("result.cell.\(row).\(column)")
            let description = value.map { $0.text == nil ? "SQL NULL" : ResultCellDisplay.preview($0) } ?? "Loading"
            cell.textField?.setAccessibilityHelp("Row \(row + 1), \(model.columns[column].name): \(description)")
            cell.setAccessibilityLabel("Row \(row + 1), \(model.columns[column].name): \(description)")
            cell.cellSelected = model.grid?.state.contains(row: row, column: column + 1) == true
            cell.cursor = model.grid?.state.cursor == ResultPosition(row: row, column: column + 1)
            return cell
        }
        func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? { ResultGridRowView() }
        func tableViewSelectionDidChange(_ notification: Notification) {
            guard !applying, let table else { return }
            if let state = model.grid?.state {
                var rows = IndexSet()
                for range in state.ranges { rows.insert(integersIn: range.start..<range.end) }
                if rows == table.selectedRowIndexes { return }
            }
            cancelRestoration()
            model.select(row: table.selectedRow, column: min(table.focusedColumn, max(0, columns.count - 1)))
            if table.selectedRow >= 0 { performGrid("cell", ["row": table.selectedRow, "column": table.focusedColumn + 1, "extend": false]) }
            rememberPosition()
            table.enclosingScrollView?.verticalRulerView?.needsDisplay = true
        }
        func focus() {
            cancelRestoration()
            guard let table, let window = table.window, window.attachedSheet == nil else { return }
            if model.selected == nil, model.availableRows > 0, table.numberOfColumns > 0 {
                let visible = table.rows(in: table.visibleRect)
                let row = visible.location == NSNotFound ? 0 : min(max(0, visible.location), model.availableRows - 1)
                let column = table.gridVisibleColumns.first ?? 0
                model.select(row: row, column: column)
                table.focusedColumn = column
                table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
            }
            if let selected = model.selected {
                table.scrollRowToVisible(selected.row)
                if table.grid != nil { table.revealGridColumn(table.grid?.state.cursor?.column ?? selected.column + 1) }
                else { table.scrollColumnToVisible(selected.column) }
            }
            window.makeFirstResponder(table)
        }

        @objc func inspectCell() { inspect?() }
        @objc func scrolled() {
            if !applying {
                if let type = NSApp.currentEvent?.type, [.scrollWheel, .leftMouseDown, .leftMouseDragged].contains(type) {
                    cancelRestoration()
                }
                restorePosition()
                rememberPosition()
            }
            table?.enclosingScrollView?.verticalRulerView?.needsDisplay = true
            scheduleVisible()
        }

        func rememberPosition() {
            guard !applying, pendingPosition == nil, let scope, !columns.isEmpty,
                  let table, table.numberOfRows > 0, let clip = table.enclosingScrollView?.contentView else { return }
            model.tablePositions[scope.document] = ResultTablePosition(headers: columns, origin: clip.bounds.origin,
                widths: table.tableColumns.map(\.width), row: table.selectedRow >= 0 ? table.selectedRow : nil,
                column: table.focusedColumn, sizing: sizing, rowNumberSelected: table.rowNumberSelected)
        }

        func restorePosition() {
            guard scope == model.scope, columns == model.columns, model.loadError == nil, let position = pendingPosition ?? layoutPosition, !columns.isEmpty,
                  let table, let scroll = table.enclosingScrollView else { return }
            guard let decision = model.restoration(for: position) else { return }
            switch decision.restoration {
            case .schemaChanged:
                cancelRestoration()
                if let scope { model.tablePositions.removeValue(forKey: scope.document) }
                return
            case .apply: break
            default: return
            }
            let wasApplying = applying
            applying = true
            defer { applying = wasApplying }
            layoutPosition = position
            if pendingPosition != nil {
                sizing = position.sizing ?? position.widths.map { ResultColumnSizing(preferred: $0, assigned: $0, manual: true, sampled: true) }
                for (column, width) in zip(table.tableColumns, position.widths) { column.width = width }
            }
            table.tile()
            let clip = scroll.contentView
            let maximumY = max(0, table.rect(ofRow: model.availableRows - 1).maxY - clip.bounds.height)
            let maximumX = max(0, table.bounds.width - clip.bounds.width)
            clip.scroll(to: NSPoint(x: min(position.origin.x, maximumX), y: min(position.origin.y, maximumY)))
            scroll.reflectScrolledClipView(clip)
            if let selected = decision.selection {
                table.focusedColumn = selected.column
                table.rowNumberSelected = position.rowNumberSelected
                table.selectRowIndexes(IndexSet(integer: selected.row), byExtendingSelection: false)
            }
            if model.isFinished || position.origin.y <= maximumY && !decision.selectionPending {
                pendingPosition = nil
            }
        }

        func cancelRestoration() { pendingPosition = nil; layoutPosition = nil; model.stopGridRestoration() }

        func tableViewColumnDidResize(_ notification: Notification) {
            guard !applying else { return }
            cancelRestoration()
            sizeColumns()
            rememberPosition()
        }

        func sizeColumns() {
            guard scope == model.scope, columns == model.columns, let table, table.tableColumns.count == columns.count else { return }
            if sizing.count != columns.count {
                sizing = table.tableColumns.enumerated().map { index, column in
                    ResultColumnSizing(preferred: max(60, (columns[index].name as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 12)]).width + 44), assigned: column.width)
                }
            }
            let rows = min(16, model.availableRows)
            for index in sizing.indices {
                if abs(table.tableColumns[index].width - sizing[index].assigned) > 1 {
                    sizing[index].manual = true
                    sizing[index].assigned = table.tableColumns[index].width
                }
                if !sizing[index].manual && !sizing[index].sampled && rows > 0 && model.value(row: 0, column: index) != nil {
                    let values = (0..<rows).compactMap { model.value(row: $0, column: index) }
                    if values.count == rows {
                        let natural = values.map { (ResultCellDisplay.preview($0) as NSString).size(withAttributes: [.font: NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)]).width + 40 }.max() ?? 60
                        sizing[index].preferred = max(sizing[index].preferred, natural)
                        sizing[index].sampled = true
                    }
                }
            }
            let wasApplying = applying
            applying = true
            defer { applying = wasApplying }
            let widths = ResultColumnSizing.fit(sizing, available: table.enclosingScrollView?.contentView.bounds.width ?? 0)
            for index in widths.indices {
                sizing[index].assigned = widths[index]
                if abs(table.tableColumns[index].width - widths[index]) > 0.5 { table.tableColumns[index].width = widths[index] }
            }
        }

        func configureGrid() {
            guard let table else { return }
            table.grid = model.grid
            table.gridChanged = { [weak self] action, arguments in self?.performGrid(action, arguments) }
            table.gridValue = { [weak model] row, column in model?.value(row: row, column: column) }
            table.copySelection = { [weak model] in model?.copyGrid() }
            model.gridAction = { [weak self] action in
                guard let self, let table = self.table else { return }
                self.focus()
                if action == "copy" { self.model.copyGrid() }
                else if action == "freeze" { self.performGrid("freeze", ["column": self.model.grid?.state.cursor?.column ?? table.focusedColumn + 1]) }
                else { self.performGrid("toggle_rows", ["visible": [0] + table.gridVisibleColumns.map { $0 + 1 }]) }
            }
        }

        func performGrid(_ action: String, _ arguments: [String: Any] = [:]) {
            guard let grid = model.grid, let table else { return }
            if action == "freeze", arguments["column"] as? Int == 0 { return }
            cancelRestoration()
            do {
                let state = try grid.apply(action, arguments)
                let wasApplying = applying
                applying = true
                defer { applying = wasApplying }
                if let cursor = state.cursor {
                    model.select(row: cursor.row, column: max(0, cursor.column - 1))
                    table.focusedColumn = max(0, cursor.column - 1)
                    table.rowNumberSelected = cursor.column == 0 || state.mode == "rows"
                }
                var rows = IndexSet()
                for range in state.ranges { rows.insert(integersIn: range.start..<range.end) }
                table.selectRowIndexes(rows, byExtendingSelection: false)
                table.reloadData()
                table.headerView?.needsDisplay = true
                table.enclosingScrollView?.verticalRulerView?.needsDisplay = true
                if ["move", "cell"].contains(action), let cursor = state.cursor {
                    table.scrollRowToVisible(cursor.row)
                    table.revealGridColumn(cursor.column)
                }
                (table.enclosingScrollView as? ResultScrollView)?.refreshFrozen()
                rememberPositionAfterGrid()
                scheduleVisible()
            } catch { model.reportGridError(error) }
        }

        private func rememberPositionAfterGrid() {
            let wasApplying = applying
            applying = false
            rememberPosition()
            applying = wasApplying
        }

        func syncGrid() {
            guard let table, let grid = model.grid else { return }
            if let selected = model.selected {
                if grid.state.mode == "rows" {
                    if grid.state.cursor?.row != selected.row { _ = try? grid.apply("row", ["row": selected.row, "gesture": "replace"]) }
                } else if grid.state.cursor?.row != selected.row || grid.state.cursor?.column != selected.column + 1 && grid.state.cursor?.column != 0 {
                    _ = try? grid.apply("cell", ["row": selected.row, "column": selected.column + 1, "extend": false])
                }
            }
            if grid.state.cursor != nil {
                var rows = IndexSet()
                for range in grid.state.ranges { rows.insert(integersIn: range.start..<range.end) }
                table.selectRowIndexes(rows, byExtendingSelection: false)
            }
            (table.enclosingScrollView as? ResultScrollView)?.refreshFrozen()
        }

        func scheduleVisible() {
            guard !scheduled else { return }
            scheduled = true
            Task { [weak self] in
                guard let self else { return }
                self.scheduled = false
                guard let table = self.table else { return }
                let rows = table.rows(in: table.visibleRect)
                let columns = table.columnIndexes(in: table.visibleRect)
                guard rows.location != NSNotFound, rows.length > 0, let first = columns.first, let last = columns.last else { return }
                #if DZBAN_TEST_FIXTURES
                if table.numberOfColumns >= 128, !self.reportedLayout {
                    self.reportedLayout = true
                    writeViewportLayout([
                        "tableWidth": table.frame.width, "tableHeight": table.frame.height,
                        "visibleWidth": table.visibleRect.width, "visibleHeight": table.visibleRect.height,
                        "scrollWidth": table.enclosingScrollView?.bounds.width ?? 0,
                        "scrollHeight": table.enclosingScrollView?.bounds.height ?? 0,
                        "visibleRows": rows.length, "visibleColumns": columns.count,
                        "totalColumns": table.numberOfColumns,
                    ])
                }
                #endif
                if columns != self.visibleColumns {
                    self.visibleColumns = columns
                    self.applying = true
                    table.reloadData(forRowIndexes: IndexSet(integersIn: rows.location..<(rows.location + rows.length)), columnIndexes: columns)
                    self.applying = false
                }
                self.model.show(rows: rows.location..<(rows.location + rows.length), columns: first..<(last + 1))
            }
        }
    }
}

public enum ResultCellDisplay {
    public static func preview(_ value: ViewportCell) -> String {
        guard let text = value.text else { return "NULL" }
        return text.replacingOccurrences(of: "\r", with: "\\r").replacingOccurrences(of: "\n", with: "\\n").replacingOccurrences(of: "\t", with: "\\t") + (value.truncated ? "..." : "")
    }
}

@MainActor
final class NativeResultTable: NSTableView {
    var didLayout: (() -> Void)?
    var userNavigated: (() -> Void)?
    var grid: ResultGrid?
    var gridChanged: ((String, [String: Any]) -> Void)?
    var gridValue: ((Int, Int) -> ViewportCell?)?
    var copySelection: (() -> Void)?
    override func layout() { super.layout(); didLayout?() }
    private var accessibleColumns: [Int: NSAccessibilityElement] = [:]
    private func accessibleArray(_ attribute: NSAccessibility.Attribute) -> [Any]? {
        switch attribute {
        case .children: return accessibilityChildren()
        case .rows, .visibleRows: return accessibilityRows()
        case .columns, .visibleColumns: return accessibilityColumns()
        default: return nil
        }
    }
    nonisolated override func accessibilityArrayAttributeCount(_ attribute: NSAccessibility.Attribute) -> Int {
        MainActor.assumeIsolated { accessibleArray(attribute)?.count ?? super.accessibilityArrayAttributeCount(attribute) }
    }
    private struct AccessibilityArray: @unchecked Sendable { let values: [Any] }
    nonisolated override func accessibilityArrayAttributeValues(_ attribute: NSAccessibility.Attribute, index: Int, maxCount: Int) -> [Any] {
        MainActor.assumeIsolated {
            guard let values = accessibleArray(attribute) else { return AccessibilityArray(values: super.accessibilityArrayAttributeValues(attribute, index: index, maxCount: maxCount)) }
            return AccessibilityArray(values: Array(values.dropFirst(max(0, index)).prefix(max(0, maxCount))))
        }.values
    }
    override func accessibilityRows() -> [any NSAccessibilityRow]? {
        let range = rows(in: visibleRect)
        guard range.location != NSNotFound else { return [] }
        return (range.location..<(range.location + range.length)).compactMap { rowView(atRow: $0, makeIfNecessary: false) }
    }
    override func accessibilityVisibleRows() -> [any NSAccessibilityRow]? { accessibilityRows() }
    override func accessibilityChildren() -> [Any]? {
        (headerView.map { [$0] } ?? []) + (accessibilityColumns() ?? [])
    }
    override func accessibilityColumns() -> [Any]? {
        let rows = rows(in: visibleRect)
        guard rows.location != NSNotFound else { return [] }
        let visible = columnIndexes(in: dataVisibleRect)
        accessibleColumns = accessibleColumns.filter { visible.contains($0.key) }
        return visible.map { column in
            let element = accessibleColumns[column] ?? NSAccessibilityElement()
            accessibleColumns[column] = element
            element.setAccessibilityElement(true)
            element.setAccessibilityRole(.column)
            element.setAccessibilityParent(self)
            element.setAccessibilityLabel(tableColumns[column].title)
            element.setAccessibilityIndex(column)
            let cells = (rows.location..<(rows.location + rows.length)).compactMap { row in
                (view(atColumn: column, row: row, makeIfNecessary: false) as? NSTableCellView)?.textField
            }
            for cell in cells { cell.setAccessibilityParent(element) }
            element.setAccessibilityChildren(cells)
            if let window {
                element.setAccessibilityFrame(window.convertToScreen(convert(rect(ofColumn: column).intersection(dataVisibleRect), to: nil)))
            }
            return element
        }
    }
    override func accessibilityVisibleColumns() -> [Any]? { accessibilityColumns() }
    override func accessibilityRowCount() -> Int { numberOfRows }
    override func accessibilityColumnCount() -> Int { numberOfColumns }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        NotificationCenter.default.removeObserver(self, name: NSApplication.didResignActiveNotification, object: nil)
        if window != nil { NotificationCenter.default.addObserver(self, selector: #selector(resetPhysicalPeek), name: NSApplication.didResignActiveNotification, object: nil) }
    }
    @objc private func resetPhysicalPeek() { resetPeek() }
    private var peekMonitor: Any?
    private var peekStarted: TimeInterval?
    var peekRow: (() -> Void)?
    var releaseRow: ((UInt64) -> Void)?
    func resetPeek() {
        if let peekMonitor { NSEvent.removeMonitor(peekMonitor) }
        peekMonitor = nil
        peekStarted = nil
    }
    private struct PeekEventResult: @unchecked Sendable { let event: NSEvent? }
    private func beginPeek(_ event: NSEvent) {
        resetPeek()
        peekStarted = event.timestamp
        peekMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp]) { [weak self] event in
            MainActor.assumeIsolated {
                guard let self else { return PeekEventResult(event: event) }
                return PeekEventResult(event: self.routePeekEvent(event))
            }.event
        }
        peekRow?()
    }

    func routePeekEvent(_ event: NSEvent) -> NSEvent? {
        guard [.keyDown, .keyUp].contains(event.type), event.keyCode == 49, let start = peekStarted else { return event }
        if event.type == .keyDown {
            if event.isARepeat { return nil }
            resetPeek()
            return event
        }
        releaseRow?(UInt64(max(0, event.timestamp - start) * 1000))
        resetPeek()
        return nil
    }
    var dataVisibleRect: NSRect {
        var rect = visibleRect
        if let ruler = enclosingScrollView?.verticalRulerView {
            let left = max(rect.minX, convert(ruler.bounds, from: ruler).maxX)
            rect = NSRect(x: left, y: rect.minY, width: max(0, rect.maxX - left), height: rect.height)
        }
        return rect
    }
    var rowPreviewAnchor: (view: NSView, rect: NSRect)? {
        guard selectedRow >= 0, selectedRow < numberOfRows, numberOfColumns > 0 else { return nil }
        scrollRowToVisible(selectedRow)
        if rowNumberSelected, let ruler = enclosingScrollView?.verticalRulerView {
            var rect = ruler.convert(rect(ofRow: selectedRow), from: self)
            rect.origin.x = ruler.bounds.minX
            rect.size.width = ruler.bounds.width
            return (ruler, rect.intersection(ruler.visibleRect))
        }
        if let anchor = (enclosingScrollView as? ResultScrollView)?.frozenAnchor(row: selectedRow, column: focusedColumn) { return anchor }
        let cell = frameOfCell(atColumn: focusedColumn, row: selectedRow).intersection(dataVisibleRect)
        return cell.isEmpty ? nil : (self, cell)
    }
    var rowNumberSelected = false
    func focusGrid() {
        window?.makeFirstResponder(self)
        if grid?.state.cursor == nil, numberOfRows > 0, numberOfColumns > 0 {
            gridChanged?("cell", ["row": 0, "column": (gridVisibleColumns.first ?? 0) + 1, "extend": false])
        }
    }
    func selectNumberedRow(_ row: Int, modifiers: NSEvent.ModifierFlags = []) {
        if grid != nil {
            window?.makeFirstResponder(self)
            gridChanged?("row", ["row": row, "gesture": Self.gesture(modifiers)])
            return
        }
        guard row >= 0, row < numberOfRows else { return }
        rowNumberSelected = true
        focusedColumn = columnIndexes(in: dataVisibleRect).first ?? 0
        window?.makeFirstResponder(self)
        selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        selectCell?(row, focusedColumn)
        reloadData(forRowIndexes: IndexSet(integer: row), columnIndexes: columnIndexes(in: visibleRect))
        enclosingScrollView?.verticalRulerView?.needsDisplay = true
    }
    var focusedColumn = 0
    var canPeek = false
    var selectCell: ((Int, Int) -> Void)?
    var inspectCell: (() -> Void)?
    var copyRows: ((IndexSet) -> Void)?
    var cancelQuery: (() -> Bool)?
    override func mouseDown(with event: NSEvent) {
        userNavigated?()
        rowNumberSelected = false
        let point = convert(event.locationInWindow, from: nil)
        let row = row(at: point)
        let column = column(at: point)
        if column >= 0 { focusedColumn = column }
        if grid != nil, row < 0 || column < 0 { focusGrid(); return }
        if row >= 0, column >= 0, grid != nil {
            window?.makeFirstResponder(self)
            gridChanged?("cell", ["row": row, "column": column + 1, "extend": event.modifierFlags.contains(.shift)])
            if event.clickCount == 2 { inspectCell?() }
        } else {
            super.mouseDown(with: event)
            selectCell?(row, column)
        }
    }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if window?.firstResponder === self, event.modifierFlags.intersection([.command, .control, .option, .shift]) == .command {
            if event.charactersIgnoringModifiers == "a" { selectAll(nil); return true }
            if event.charactersIgnoringModifiers == "c" { copy(nil); return true }
        }
        return super.performKeyEquivalent(with: event)
    }
    override func keyDown(with event: NSEvent) {
        let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
        if grid != nil {
            if modifiers == .control && event.keyCode == 49 {
                gridChanged?("toggle_rows", ["visible": [0] + gridVisibleColumns.map { $0 + 1 }]); return
            }
            if modifiers.isEmpty && event.charactersIgnoringModifiers == "x" {
                gridChanged?("freeze", ["column": grid?.state.cursor?.column ?? focusedColumn + 1]); return
            }
            if modifiers.isEmpty || modifiers == .shift {
                let delta: (Int, Int)?
                switch event.keyCode {
                case 123: delta = (-1, 0)
                case 124: delta = (1, 0)
                case 125: delta = (0, 1)
                case 126: delta = (0, -1)
                case 116: delta = (0, -max(1, Int(visibleRect.height / rowHeight)))
                case 121: delta = (0, max(1, Int(visibleRect.height / rowHeight)))
                case 115: delta = (0, -numberOfRows)
                case 119: delta = (0, numberOfRows)
                default: delta = nil
                }
                if let delta { gridChanged?("move", ["horizontal": delta.0, "vertical": delta.1, "extend": modifiers == .shift]); return }
            }
        }
        let plain = event.modifierFlags.intersection([.command, .control, .option, .shift]).isEmpty
        if plain && [115, 116, 119, 121, 123, 124, 125, 126].contains(event.keyCode) { userNavigated?() }
        if plain && event.keyCode == 49, canPeek { if !event.isARepeat { beginPeek(event) }; return }
        if plain && event.keyCode == 53, cancelQuery?() == true { return }
        if plain && (event.keyCode == 123 || event.keyCode == 124), selectedRow >= 0, numberOfColumns > 0 {
            rowNumberSelected = false
            focusedColumn = min(max(0, focusedColumn + (event.keyCode == 123 ? -1 : 1)), numberOfColumns - 1)
            selectCell?(selectedRow, focusedColumn)
            scrollColumnToVisible(focusedColumn)
            return
        }
        super.keyDown(with: event)
    }
    override func selectAll(_ sender: Any?) {
        if grid != nil { gridChanged?("all", [:]); return }
        selectRowIndexes(IndexSet(integersIn: 0..<numberOfRows), byExtendingSelection: false)
    }
    @objc func copy(_ sender: Any?) { if grid != nil { copySelection?() } else if !selectedRowIndexes.isEmpty { copyRows?(selectedRowIndexes) } }
    override func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        if item.action == #selector(selectAll(_:)) { return numberOfRows > 0 }
        if item.action == #selector(copy(_:)) { return !selectedRowIndexes.isEmpty }
        return super.validateUserInterfaceItem(item)
    }
}

@MainActor
private final class ResultCellView: NSTableCellView {
    var leadingInset: CGFloat = WorkspaceLayout.columnInset
    var trailingInset: CGFloat = WorkspaceLayout.columnInset
    var cursor = false { didSet { if oldValue != cursor { needsDisplay = true } } }
    var cellSelected = false { didSet { if oldValue != cellSelected { needsDisplay = true } } }
    override func layout() {
        super.layout()
        textField?.frame = NSRect(x: bounds.minX + leadingInset, y: bounds.minY + 3, width: max(0, bounds.width - leadingInset - trailingInset), height: max(0, bounds.height - 6))
    }
    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        if cellSelected { NSColor.controlAccentColor.withAlphaComponent(0.18).setFill(); bounds.fill() }
        if cursor {
            NSColor.controlAccentColor.setStroke()
            let border = NSBezierPath(rect: bounds.insetBy(dx: 1, dy: 1))
            border.lineWidth = 2
            border.stroke()
        }
    }
}

@MainActor
private final class ResultColumnHeaderCell: NSTableHeaderCell {
    var leadingInset: CGFloat = WorkspaceLayout.columnInset
    var trailingInset: CGFloat = WorkspaceLayout.columnInset
    override func draw(withFrame cellFrame: NSRect, in controlView: NSView) {
        NSColor.controlBackgroundColor.setFill()
        cellFrame.fill()
        let font = NSFont.systemFont(ofSize: 12, weight: .medium)
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineBreakMode = .byTruncatingTail
        let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: NSColor.labelColor, .paragraphStyle: paragraph]
        let height = (stringValue as NSString).size(withAttributes: attributes).height
        if let table = (controlView as? NSTableHeaderView)?.tableView as? NativeResultTable,
           let column = table.tableColumns.firstIndex(where: { $0.headerCell === self }) {
            if table.grid?.state.mode == "columns", table.grid?.state.columns.contains(column + 1) == true {
                NSColor.controlAccentColor.withAlphaComponent(0.2).setFill(); cellFrame.fill()
            }
            if table.grid?.state.frozen.contains(column + 1) == true {
                NSImage(systemSymbolName: "lock.fill", accessibilityDescription: "Frozen column")?.draw(in: NSRect(x: cellFrame.maxX - 18, y: cellFrame.midY - 6, width: 12, height: 12))
            }
        }
        let textRect = NSRect(x: cellFrame.minX + leadingInset, y: cellFrame.midY - height / 2, width: max(0, cellFrame.width - leadingInset - trailingInset), height: height)
        (stringValue as NSString).draw(in: textRect, withAttributes: attributes)
        let pixel = 1 / (controlView.window?.backingScaleFactor ?? 1)
        NSColor.separatorColor.setFill()
        let edge = (cellFrame.maxX / pixel).rounded() * pixel
        NSRect(x: edge - 1, y: cellFrame.minY, width: 1, height: cellFrame.height).fill()
        NSRect(x: cellFrame.minX, y: cellFrame.maxY - pixel, width: cellFrame.width, height: pixel).fill()
    }
}

enum ResultRowNumberLayout {
    static func width(rows: Int) -> CGFloat {
        max(32, CGFloat(String(max(1, rows)).count) * 8 + 16)
    }
}

@MainActor
private final class ResultRowNumbers: NSRulerView {
    private var elements: [Int: RowNumberElement] = [:]
    private var table: NativeResultTable? { clientView as? NativeResultTable }

    override init(scrollView: NSScrollView?, orientation: NSRulerView.Orientation) {
        super.init(scrollView: scrollView, orientation: orientation)
        reservedThicknessForMarkers = 0
        reservedThicknessForAccessoryView = 0
        ruleThickness = ResultRowNumberLayout.width(rows: 0)
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel("Row numbers")
        setAccessibilityIdentifier("results.row-numbers")
    }

    required init(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func update(rows: Int) {
        let width = ResultRowNumberLayout.width(rows: rows)
        if ruleThickness != width { ruleThickness = width }
        needsDisplay = true
    }

    private var visibleRows: Range<Int> {
        guard let table else { return 0..<0 }
        let range = table.rows(in: table.visibleRect)
        guard range.location != NSNotFound else { return 0..<0 }
        return range.location..<min(table.numberOfRows, range.location + range.length)
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let table else { return }
        NSColor.controlBackgroundColor.setFill()
        bounds.fill()
        let font = NSFont.monospacedDigitSystemFont(ofSize: 12, weight: .regular)
        for row in visibleRows {
            var rect = convert(table.rect(ofRow: row), from: table)
            rect.origin.x = bounds.minX
            rect.size.width = bounds.width
            guard rect.intersects(dirtyRect) else { continue }
            let selected = table.selectedRowIndexes.contains(row)
            let background = selected ? NSColor.selectedContentBackgroundColor : NSColor.alternatingContentBackgroundColors[row % 2]
            background.setFill()
            rect.fill()
            let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: selected ? NSColor.alternateSelectedControlTextColor : NSColor.secondaryLabelColor]
            let text = String(row + 1) as NSString
            let size = text.size(withAttributes: attributes)
            text.draw(at: NSPoint(x: bounds.maxX - WorkspaceLayout.gutterInset - size.width, y: rect.midY - size.height / 2), withAttributes: attributes)
        }
        NSColor.separatorColor.setFill()
        NSRect(x: bounds.maxX - 1, y: bounds.minY, width: 1, height: bounds.height).fill()
    }

    override func mouseDown(with event: NSEvent) {
        guard let table else { return }
        let point = table.convert(event.locationInWindow, from: nil)
        let row = table.row(at: NSPoint(x: 0, y: point.y))
        table.selectNumberedRow(row, modifiers: event.modifierFlags)
    }

    override func accessibilityChildren() -> [Any]? {
        guard let table, let window else { return [] }
        let rows = visibleRows
        elements = elements.filter { rows.contains($0.key) }
        return rows.map { row in
            let element = elements[row] ?? RowNumberElement()
            elements[row] = element
            element.setAccessibilityParent(self)
            element.setAccessibilityRole(.button)
            element.setAccessibilityLabel("Row \(row + 1)")
            element.setAccessibilityIdentifier("result.row-number.\(row)")
            element.setAccessibilityValue(String(row + 1))
            element.setAccessibilitySelected(table.selectedRowIndexes.contains(row))
            element.setAccessibilityHelp("Select row. Space opens its preview.")
            var rect = convert(table.rect(ofRow: row), from: table)
            rect.origin.x = bounds.minX
            rect.size.width = bounds.width
            element.setAccessibilityFrame(window.convertToScreen(convert(rect.intersection(bounds), to: nil)))
            element.press = { [weak table] in table?.selectNumberedRow(row) }
            return element
        }
    }
}

@MainActor
private final class RowNumberAction {
    var press: (() -> Void)?
    nonisolated init() {}
}

@MainActor
final class RowNumberElement: NSAccessibilityElement {
    private nonisolated let action = RowNumberAction()
    var press: (() -> Void)? {
        get { action.press }
        set { action.press = newValue }
    }
    nonisolated override func accessibilityPerformPress() -> Bool {
        let action = action
        if Thread.isMainThread { MainActor.assumeIsolated { action.press?() } }
        else { DispatchQueue.main.async { action.press?() } }
        return true
    }
}

@MainActor
final class ResultScrollView: NSScrollView {
    override class var isCompatibleWithResponsiveScrolling: Bool { false }

    override func mouseDown(with event: NSEvent) {
        if let table = documentView as? NativeResultTable, table.grid != nil {
            table.focusGrid()
        } else { super.mouseDown(with: event) }
    }

    private let numberHeader = RowNumberHeader(frame: .zero)
    private let frozenLeft = ResultFrozenPane(frame: .zero)
    private let frozenRight = ResultFrozenPane(frame: .zero)
    private let frozenLeftHeader = ResultFrozenPane(frame: .zero)
    private let frozenRightHeader = ResultFrozenPane(frame: .zero)

    override init(frame: NSRect) {
        super.init(frame: frame)
        numberHeader.isHidden = true
        addSubview(numberHeader)
        for pane in [frozenLeft, frozenRight, frozenLeftHeader, frozenRightHeader] {
            pane.isHidden = true
            pane.setAccessibilityElement(true)
            pane.setAccessibilityRole(.group)
            pane.setAccessibilityIdentifier("results.frozen")
            addSubview(pane)
        }
        frozenLeftHeader.header = true
        frozenRightHeader.header = true
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func tile() {
        super.tile()
        alignHeader()
    }

    override func reflectScrolledClipView(_ clipView: NSClipView) {
        super.reflectScrolledClipView(clipView)
        if clipView === contentView {
            alignHeader()
            verticalRulerView?.needsDisplay = true
            refreshFrozen()
        }
    }

    override func accessibilityChildren() -> [Any]? {
        var children = super.accessibilityChildren() ?? []
        for pane in [frozenLeft, frozenRight, frozenLeftHeader, frozenRightHeader] where !pane.isHidden {
            if !children.contains(where: { ($0 as AnyObject) === pane }) { children.append(pane) }
        }
        return children
    }

    func frozenAnchor(row: Int, column: Int) -> (view: NSView, rect: NSRect)? {
        guard let table = documentView as? NativeResultTable else { return nil }
        for pane in [frozenLeft, frozenRight] where !pane.isHidden {
            if let index = pane.columns.firstIndex(of: column) {
                let x = pane.columns.prefix(index).reduce(CGFloat.zero) { $0 + table.tableColumns[$1].width }
                let source = pane.convert(table.rect(ofRow: row), from: table)
                let rect = NSRect(x: x, y: source.minY, width: table.tableColumns[column].width, height: source.height).intersection(pane.bounds)
                if !rect.isEmpty { return (pane, rect) }
            }
        }
        return nil
    }

    func refreshFrozen() {
        guard let table = documentView as? NativeResultTable, let layout = table.gridLayout() else { return }
        for (pane, header, columns, right) in [(frozenLeft, frozenLeftHeader, layout.left, false), (frozenRight, frozenRightHeader, layout.right, true)] {
            let actual = columns.filter { $0 > 0 }.map { $0 - 1 }
            let width = actual.reduce(CGFloat.zero) { $0 + table.tableColumns[$1].width }
            for view in [pane, header] { view.table = table; view.columns = actual; view.isHidden = actual.isEmpty; view.needsDisplay = true }
            let x = right ? contentView.frame.maxX - width : max(contentView.frame.minX, verticalRulerView?.frame.maxX ?? 0)
            pane.frame = NSRect(x: x, y: contentView.frame.minY, width: width, height: contentView.frame.height)
            header.frame = NSRect(x: x, y: numberHeader.frame.minY, width: width, height: numberHeader.frame.height)
        }
    }

    func alignHeader() {
        guard let ruler = verticalRulerView, let table = documentView as? NSTableView else { return }
        var ancestor = table.headerView?.superview
        while let view = ancestor, !(view is NSClipView) { ancestor = view.superview }
        guard let headerClip = ancestor as? NSClipView else { return }
        var frame = headerClip.frame
        frame.origin.x = ruler.frame.maxX
        frame.size.width = max(0, contentView.frame.maxX - frame.minX)
        headerClip.frame = frame
        headerClip.scroll(to: NSPoint(x: contentView.bounds.minX + frame.minX - contentView.frame.minX, y: headerClip.bounds.minY))
        numberHeader.table = table as? NativeResultTable
        numberHeader.frame = NSRect(x: ruler.frame.minX, y: frame.minY, width: frame.minX - ruler.frame.minX, height: frame.height)
        numberHeader.isHidden = false
        addSubview(numberHeader, positioned: .above, relativeTo: headerClip)
        refreshFrozen()
    }
}

@MainActor
private final class RowNumberHeader: NSView {
    nonisolated override func accessibilityPerformPress() -> Bool {
        DispatchQueue.main.async { [weak self] in
            guard let self, let table = self.table else { return }
            table.window?.makeFirstResponder(table)
            table.gridChanged?("column", ["column": 0, "gesture": "replace"])
        }
        return true
    }
    weak var table: NativeResultTable?
    override func mouseDown(with event: NSEvent) {
        table?.window?.makeFirstResponder(table)
        table?.gridChanged?("column", ["column": 0, "gesture": NativeResultTable.gesture(event.modifierFlags)])
    }
    override init(frame: NSRect) {
        super.init(frame: frame)
        setAccessibilityElement(true)
        setAccessibilityRole(.button)
        setAccessibilityLabel("Row numbers")
        setAccessibilityHelp("Select the row-number column. Row numbers stay visible while scrolling.")
        setAccessibilityValue("#")
        setAccessibilityIdentifier("results.row-number-header")
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.controlBackgroundColor.setFill()
        bounds.fill()
        let attributes: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 12, weight: .medium), .foregroundColor: NSColor.secondaryLabelColor]
        let text = "#" as NSString
        let size = text.size(withAttributes: attributes)
        text.draw(at: NSPoint(x: bounds.maxX - WorkspaceLayout.gutterInset - size.width, y: bounds.midY - size.height / 2), withAttributes: attributes)
        let pixel = 1 / (window?.backingScaleFactor ?? 1)
        NSColor.separatorColor.setFill()
        NSRect(x: bounds.maxX - 1, y: bounds.minY, width: 1, height: bounds.height).fill()
        NSRect(x: bounds.minX, y: bounds.minY, width: bounds.width, height: pixel).fill()
    }
}
