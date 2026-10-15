import AppKit
import DzbanBridge
import SwiftUI

struct ResultTable: NSViewRepresentable {
    @ObservedObject var model: ResultViewport
    let inspect: () -> Void
    let copy: () -> Void
    let cancel: () -> Bool

    func makeCoordinator() -> Coordinator { Coordinator(model: model) }
    func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSScrollView, context: Context) -> CGSize? {
        let width = proposal.width.flatMap { $0.isFinite ? $0 : nil } ?? 600
        let height = proposal.height.flatMap { $0.isFinite ? $0 : nil } ?? 240
        return CGSize(width: width, height: height)
    }
    func makeNSView(context: Context) -> NSScrollView {
        let table = NativeResultTable()
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        scroll.documentView = table
        table.delegate = context.coordinator
        table.dataSource = context.coordinator
        table.headerView = NSTableHeaderView()
        table.usesAlternatingRowBackgroundColors = true
        table.columnAutoresizingStyle = .noColumnAutoresizing
        table.allowsColumnReordering = false
        table.allowsMultipleSelection = false
        table.rowHeight = 24
        table.gridStyleMask = [.solidVerticalGridLineMask]
        table.setAccessibilityIdentifier("results.table")
        table.setAccessibilityLabel("Query results")
        table.setAccessibilityHelp("Select a cell to copy its original value. Return opens the full value inspector.")
        context.coordinator.table = table
        table.target = context.coordinator
        table.doubleAction = #selector(Coordinator.inspectCell)
        scroll.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(context.coordinator, selector: #selector(Coordinator.scrolled), name: NSView.boundsDidChangeNotification, object: scroll.contentView)
        return scroll
    }
    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let table = scroll.documentView as? NativeResultTable else { return }
        let coordinator = context.coordinator
        coordinator.inspect = inspect
        coordinator.copy = copy
        table.inspectCell = inspect
        table.copyCell = copy
        table.cancelQuery = cancel
        table.selectCell = { [weak coordinator] row, column in coordinator?.model.select(row: row, column: column) }
        table.canCopy = model.selected != nil
        coordinator.applying = true
        if coordinator.columns != model.columns {
            coordinator.columns = model.columns
            for column in table.tableColumns { table.removeTableColumn(column) }
            for (index, column) in model.columns.enumerated() {
                let native = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("result.column.\(index)"))
                native.title = column.name
                native.width = 170
                native.minWidth = 60
                table.addTableColumn(native)
            }
        }
        table.reloadData()
        if let selected = model.selected, selected.row < model.availableRows {
            if table.selectedRow != selected.row { table.selectRowIndexes(IndexSet(integer: selected.row), byExtendingSelection: false) }
            table.focusedColumn = selected.column
        } else { table.deselectAll(nil) }
        if model.availableRows > 0, let clip = table.enclosingScrollView?.contentView {
            let maximumY = max(0, table.rect(ofRow: model.availableRows - 1).maxY - clip.bounds.height)
            if clip.bounds.minY > maximumY { clip.scroll(to: NSPoint(x: clip.bounds.minX, y: maximumY)) }
        }
        coordinator.applying = false
        coordinator.scheduleVisible()
    }

    static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        NotificationCenter.default.removeObserver(coordinator)
        coordinator.table?.delegate = nil
        coordinator.table?.dataSource = nil
    }

    @MainActor
    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate {
        let model: ResultViewport
        weak var table: NativeResultTable?
        var columns: [Column] = []
        var applying = false
        var inspect: (() -> Void)?
        var copy: (() -> Void)?
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
            let value = model.value(row: row, column: column)
            let display = value.map(ResultCellDisplay.preview) ?? "..."
            cell.textField?.stringValue = display
            cell.textField?.setAccessibilityIdentifier("result.cell.\(row).\(column)")
            cell.textField?.textColor = value?.text == nil ? .secondaryLabelColor : .labelColor
            cell.setAccessibilityIdentifier("result.cell.\(row).\(column)")
            let description = value.map { $0.text == nil ? "SQL NULL" : ResultCellDisplay.preview($0) } ?? "Loading"
            cell.textField?.setAccessibilityHelp("Row \(row + 1), \(model.columns[column].name): \(description)")
            cell.setAccessibilityLabel("Row \(row + 1), \(model.columns[column].name): \(description)")
            cell.cellSelected = model.selected?.row == row && model.selected?.column == column
            return cell
        }
        func tableViewSelectionDidChange(_ notification: Notification) {
            guard !applying, let table else { return }
            model.select(row: table.selectedRow, column: min(table.focusedColumn, max(0, columns.count - 1)))
        }
        @objc func inspectCell() { inspect?() }
        @objc func scrolled() { scheduleVisible() }
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
        let visible = columnIndexes(in: visibleRect)
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
                element.setAccessibilityFrame(window.convertToScreen(convert(rect(ofColumn: column).intersection(visibleRect), to: nil)))
            }
            return element
        }
    }
    override func accessibilityVisibleColumns() -> [Any]? { accessibilityColumns() }
    override func accessibilityRowCount() -> Int { numberOfRows }
    override func accessibilityColumnCount() -> Int { numberOfColumns }

    var focusedColumn = 0
    var canCopy = false
    var selectCell: ((Int, Int) -> Void)?
    var inspectCell: (() -> Void)?
    var copyCell: (() -> Void)?
    var cancelQuery: (() -> Bool)?
    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        let row = row(at: point)
        let column = column(at: point)
        if column >= 0 { focusedColumn = column }
        super.mouseDown(with: event)
        selectCell?(row, column)
    }
    override func keyDown(with event: NSEvent) {
        let plain = event.modifierFlags.intersection([.command, .control, .option, .shift]).isEmpty
        if plain && event.keyCode == 53, cancelQuery?() == true { return }
        if plain && (event.keyCode == 36 || event.keyCode == 76), canCopy { inspectCell?(); return }
        if plain && (event.keyCode == 123 || event.keyCode == 124), selectedRow >= 0, numberOfColumns > 0 {
            focusedColumn = min(max(0, focusedColumn + (event.keyCode == 123 ? -1 : 1)), numberOfColumns - 1)
            selectCell?(selectedRow, focusedColumn)
            scrollColumnToVisible(focusedColumn)
            return
        }
        super.keyDown(with: event)
    }
    @objc func copy(_ sender: Any?) { if canCopy { copyCell?() } }
    override func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        if item.action == #selector(copy(_:)) { return canCopy }
        return super.validateUserInterfaceItem(item)
    }
}

@MainActor
private final class ResultCellView: NSTableCellView {
    var cellSelected = false { didSet { if oldValue != cellSelected { needsDisplay = true } } }
    override func layout() {
        super.layout()
        textField?.frame = bounds.insetBy(dx: 6, dy: 3)
    }
    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        if cellSelected {
            NSColor.controlAccentColor.setStroke()
            let border = NSBezierPath(rect: bounds.insetBy(dx: 1, dy: 1))
            border.lineWidth = 2
            border.stroke()
        }
    }
}
