import AppKit
import DzbanBridge

@MainActor
final class ResultGridRowView: NSTableRowView {
    override var interiorBackgroundStyle: NSView.BackgroundStyle { .normal }
    override func drawSelection(in dirtyRect: NSRect) {}
}

extension NativeResultTable {
    var gridVisibleColumns: IndexSet {
        guard let layout = gridLayout() else { return columnIndexes(in: dataVisibleRect) }
        return IndexSet(layout.visible.filter { $0 > 0 }.map { $0 - 1 })
    }

    static func gesture(_ modifiers: NSEvent.ModifierFlags) -> String {
        modifiers.contains(.shift) ? "extend" : modifiers.contains(.command) ? "toggle" : "replace"
    }

    func gridLayout(reveal column: Int? = nil) -> ResultGridState? {
        guard let grid, let scroll = enclosingScrollView else { return nil }
        let numberWidth = scroll.verticalRulerView?.ruleThickness ?? 0
        var arguments: [String: Any] = ["widths": [numberWidth] + (0..<numberOfColumns).map { rect(ofColumn: $0).width }, "scroll": dataVisibleRect.minX,
                                       "viewport": dataVisibleRect.width + numberWidth]
        if let column { arguments["column"] = column }
        return try? grid.apply("layout", arguments)
    }

    func revealGridColumn(_ column: Int) {
        guard let scroll = enclosingScrollView, let target = gridLayout(reveal: column)?.reveal else { return }
        scroll.contentView.scroll(to: NSPoint(x: scroll.contentView.bounds.minX + target - dataVisibleRect.minX, y: scroll.contentView.bounds.minY))
        scroll.reflectScrolledClipView(scroll.contentView)
    }
}

@MainActor
final class ResultGridHeaderView: NSTableHeaderView {
    private var elements: [Int: RowNumberElement] = [:]
    override func accessibilityChildren() -> [Any]? {
        guard let table = tableView as? NativeResultTable, let grid = table.grid, let window else { return super.accessibilityChildren() }
        let visible = table.columnIndexes(in: table.dataVisibleRect)
        elements = elements.filter { visible.contains($0.key) }
        return visible.map { column in
            let element = elements[column] ?? RowNumberElement()
            elements[column] = element
            element.setAccessibilityParent(self)
            element.setAccessibilityRole(.button)
            element.setAccessibilityIdentifier("result.header.\(column)")
            element.setAccessibilityLabel(table.tableColumns[column].title)
            element.setAccessibilityHelp(grid.state.frozen.contains(column + 1) ? "Frozen column. Select all values in this column." : "Select all values in this column.")
            element.setAccessibilitySelected(grid.state.mode == "columns" && grid.state.columns.contains(column + 1))
            element.setAccessibilityFrame(window.convertToScreen(convert(headerRect(ofColumn: column).intersection(visibleRect), to: nil)))
            element.press = { [weak table] in
                guard let table else { return }
                table.window?.makeFirstResponder(table)
                table.gridChanged?("column", ["column": column + 1, "gesture": "replace"])
            }
            return element
        }
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        let column = column(at: point)
        guard let table = tableView as? NativeResultTable, table.grid != nil, column >= 0,
              point.x > headerRect(ofColumn: column).minX + 6, point.x < headerRect(ofColumn: column).maxX - 6 else {
            super.mouseDown(with: event); return
        }
        table.window?.makeFirstResponder(table)
        table.gridChanged?("column", ["column": column + 1, "gesture": NativeResultTable.gesture(event.modifierFlags)])
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        let column = column(at: convert(event.locationInWindow, from: nil))
        guard column >= 0, let table = tableView as? NativeResultTable else { return nil }
        let menu = NSMenu()
        let frozen = table.grid?.state.frozen.contains(column + 1) == true
        let item = NSMenuItem(title: frozen ? "Unfreeze Column" : "Freeze Column", action: #selector(toggleFrozen(_:)), keyEquivalent: "")
        item.target = self
        item.tag = column + 1
        menu.addItem(item)
        return menu
    }
    @objc private func toggleFrozen(_ item: NSMenuItem) { (tableView as? NativeResultTable)?.gridChanged?("freeze", ["column": item.tag]) }
}

@MainActor
final class ResultFrozenPane: NSView {
    weak var table: NativeResultTable?
    var columns: [Int] = []
    var header = false
    private var elements: [String: RowNumberElement] = [:]
    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        guard let table else { return }
        NSColor.controlBackgroundColor.setFill()
        bounds.fill()
        var x: CGFloat = 0
        for column in columns {
            let width = table.tableColumns[column].width
            if header {
                let rect = NSRect(x: x, y: 0, width: width, height: bounds.height)
                table.tableColumns[column].headerCell.draw(withFrame: rect, in: table.headerView ?? table)
            } else {
                let rows = table.rows(in: table.visibleRect)
                if rows.location != NSNotFound {
                    for row in rows.location..<min(table.numberOfRows, rows.location + rows.length) {
                        let source = table.rect(ofRow: row)
                        let rect = NSRect(x: x, y: convert(source, from: table).minY, width: width, height: source.height)
                        guard rect.intersects(dirtyRect) else { continue }
                        let selected = table.grid?.state.contains(row: row, column: column + 1) == true
                        (selected ? NSColor.controlAccentColor.withAlphaComponent(0.18) : NSColor.alternatingContentBackgroundColors[row % 2]).setFill()
                        rect.fill()
                        let value = table.gridValue?(row, column)
                        let text = value.map(ResultCellDisplay.preview) ?? "..."
                        let leading = column == 0 ? WorkspaceLayout.gutterInset : WorkspaceLayout.columnInset
                        let trailing = column == table.numberOfColumns - 1 ? WorkspaceLayout.textInset : WorkspaceLayout.columnInset
                        let textRect = NSRect(x: rect.minX + leading, y: rect.minY + 3, width: max(0, rect.width - leading - trailing), height: max(0, rect.height - 6))
                        let paragraph = NSMutableParagraphStyle()
                        paragraph.lineBreakMode = .byTruncatingTail
                        (text as NSString).draw(in: textRect, withAttributes: [.font: NSFont.monospacedSystemFont(ofSize: 12, weight: .regular), .foregroundColor: value?.text == nil ? NSColor.secondaryLabelColor : NSColor.labelColor, .paragraphStyle: paragraph])
                        if table.grid?.state.cursor == ResultPosition(row: row, column: column + 1) {
                            NSColor.controlAccentColor.setStroke()
                            let border = NSBezierPath(rect: rect.insetBy(dx: 1, dy: 1))
                            border.lineWidth = 2
                            border.stroke()
                        }
                    }
                }
            }
            NSColor.separatorColor.setFill()
            NSRect(x: x + width - 1, y: 0, width: 1, height: bounds.height).fill()
            x += width
        }
    }

    private func column(at point: NSPoint) -> Int? {
        guard let table else { return nil }
        var x: CGFloat = 0
        for column in columns {
            x += table.tableColumns[column].width
            if point.x < x { return column }
        }
        return nil
    }
    override func mouseDown(with event: NSEvent) {
        guard let table, let column = column(at: convert(event.locationInWindow, from: nil)) else { return }
        table.window?.makeFirstResponder(table)
        if header {
            table.gridChanged?("column", ["column": column + 1, "gesture": NativeResultTable.gesture(event.modifierFlags)])
        } else {
            let row = table.row(at: table.convert(event.locationInWindow, from: nil))
            guard row >= 0 else { return }
            table.gridChanged?("cell", ["row": row, "column": column + 1, "extend": event.modifierFlags.contains(.shift)])
            if event.clickCount == 2 { table.inspectCell?() }
        }
    }
    override func scrollWheel(with event: NSEvent) { table?.enclosingScrollView?.scrollWheel(with: event) }
    override func menu(for event: NSEvent) -> NSMenu? {
        guard let column = column(at: convert(event.locationInWindow, from: nil)) else { return nil }
        let menu = NSMenu()
        let item = NSMenuItem(title: "Unfreeze Column", action: #selector(unfreeze(_:)), keyEquivalent: "")
        item.target = self
        item.tag = column + 1
        menu.addItem(item)
        return menu
    }
    @objc private func unfreeze(_ item: NSMenuItem) { table?.gridChanged?("freeze", ["column": item.tag]) }

    override func accessibilityChildren() -> [Any]? {
        guard let table, let window else { return [] }
        let rows = table.rows(in: table.visibleRect)
        guard rows.location != NSNotFound else { return [] }
        var children: [Any] = []
        var retained: [String: RowNumberElement] = [:]
        var x: CGFloat = 0
        for column in columns {
            let width = table.tableColumns[column].width
            if header {
                let key = "header.\(column)"
                let element = elements[key] ?? RowNumberElement()
                retained[key] = element
                element.setAccessibilityParent(self)
                element.setAccessibilityRole(.button)
                element.setAccessibilityIdentifier("result.frozen.header.\(column)")
                element.setAccessibilityLabel("Frozen column: \(table.tableColumns[column].title)")
                element.setAccessibilityFrame(window.convertToScreen(convert(NSRect(x: x, y: 0, width: width, height: bounds.height), to: nil)))
                element.press = { [weak table] in
                    guard let table else { return }
                    table.window?.makeFirstResponder(table)
                    table.gridChanged?("column", ["column": column + 1, "gesture": "replace"])
                }
                children.append(element)
            } else {
                for row in rows.location..<min(table.numberOfRows, rows.location + rows.length) {
                    let key = "\(row).\(column)"
                    let element = elements[key] ?? RowNumberElement()
                    retained[key] = element
                    element.setAccessibilityParent(self)
                    element.setAccessibilityRole(.button)
                    element.setAccessibilityIdentifier("result.frozen.cell.\(row).\(column)")
                    element.setAccessibilityLabel("Frozen \(table.tableColumns[column].title), row \(row + 1)")
                    let value = table.gridValue?(row, column)
                    element.setAccessibilityValue(value.map(ResultCellDisplay.preview) ?? "Loading")
                    element.setAccessibilityHelp(value == nil ? "Loading" : value?.text == nil ? "SQL NULL" : "Text value")
                    element.setAccessibilitySelected(table.grid?.state.contains(row: row, column: column + 1) == true)
                    var rect = convert(table.rect(ofRow: row), from: table)
                    rect.origin.x = x
                    rect.size.width = width
                    element.setAccessibilityFrame(window.convertToScreen(convert(rect.intersection(bounds), to: nil)))
                    element.press = { [weak table] in
                        guard let table else { return }
                        table.window?.makeFirstResponder(table)
                        table.gridChanged?("cell", ["row": row, "column": column + 1, "extend": false])
                    }
                    children.append(element)
                }
            }
            x += width
        }
        elements = retained
        return children
    }
}
