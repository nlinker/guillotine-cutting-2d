Attribute VB_Name = "Cut"
''
'' Cut.bas  -  VBA module: launches solver.exe and reads progress via named pipe
''
'' Requires JsonConverter module (excel/JsonConverter.bas) imported into the workbook.
''
'' Usage:
''   1. Alt+F11 -> Import File -> Сut.bas + JsonConverter.bas
''   2. Fill in data on Sheet1
''   3. Run macro RunCut
'' ============================================================================

Option Explicit

'' == Constants ================================================================

Private Const MAIN_SHEET_INDEX As Long   = 1     ' tab position of the input sheet; addressed by index (1-based), not name, so renaming the tab doesn't break anything
Private Const EXTR_SHEET_INDEX As Long   = 2     ' tab position of the "Выписка" (extract) sheet

Private Const PIPE_NAME        As String = "\\.\pipe\cut_progress"
Private Const GENERIC_READ     As Long   = &H80000000
Private Const OPEN_EXISTING    As Long   = 3
Private Const FILE_ATTR_NORMAL As Long   = &H80
Private Const BUFFER_SIZE      As Long   = 8192
Private Const MAX_PIECE_ROWS   As Long   = 100   ' max rows in the piece table

Private Const SHEET_W_CELL     As String = "K1"  ' sheet width (mm)
Private Const SHEET_H_CELL     As String = "L1"  ' sheet height (mm)
Private Const KERF_CELL        As String = "L2"  ' blade kerf width (mm)
Private Const MARGIN_CELL      As String = "L3"  ' edge margin (mm)
Private Const ACAD_FONT_SIZE   As String = "L4"  ' label text height in drawing units (mm)
Private Const EDGE_MARGIN_CELL As String = "L5"  ' edging overhang per strip (mm); default 40
Private Const DATA_CELL        As String = "A8"  ' top-left of piece table ("Panel" label column, first input row)
Private Const RESULT_CELL      As String = "P8"  ' top-left of placement table ("Sheet" label column, first result row)

Private Const CFG_RANDOM_SEED_CHK As String = "ChkRandomSeed"  ' checkbox: randomize seed on each run

Private Const CFG_SEED_CELL           As String = "O1"  ' base random seed (--seed)
Private Const CFG_GENS_CELL           As String = "O2"  ' generations per run (--iterations)
Private Const CFG_POP_CELL            As String = "O3"  ' population size (--pop)
Private Const CFG_ALGORITHM_CELL      As String = "O4"  ' algorithm: glas/bfdh/jylanki
Private Const CFG_LARGE_AREA_CELL     As String = "O5"  ' large_area_threshold (0 = auto)
Private Const CFG_LONG_DIM_CELL       As String = "O6"  ' long_dim_threshold   (0 = auto)
Private Const OUT_STATUS_CELL  As String = "S1"  ' status text
Private Const OUT_GEN_CELL     As String = "S2"  ' current generation
Private Const OUT_OBJ_CELL     As String = "S3"  ' best objective
Private Const OUT_SHEETS_CELL  As String = "S4"  ' sheets used
Private Const OUT_CUT_CELL     As String = "S5"  ' cuts used for each of the sheets

Private Const CANVAS_RANGE     As String = "J8:N8"  ' top row of canvas; spans the full drawn width
Private Const CANVAS_SHEET_GAP As Double = 14#   ' gap between sheets in points
Private Const SHEET_GAP_ACAD   As Long   = 150   ' gap between sheets exported to AutoCAD (drawing units)

Private Const EXTR_LEFT_CELL  As String = "A2"  ' header row of the left block (data starts one row below)
Private Const EXTR_RIGHT_CELL As String = "H2"  ' header row of the right block (G = 1-column gap)
Private Const EXTR_COLS       As Long   = 6     ' Панель, N п/п, Длина, Ширина, Кол-во, Примечание
Private Const EXTR_SHAPE_PREFIX As String = "extr_edge_"
Private Const EDGE_DASH_COUNT   As Long   = 3     ' number of dashes drawn per dashed edge bar, regardless of cell width
Private Const EDGE_DASH_GAP_RATIO As Double = 4 / 9  ' fraction of each dash+gap unit that is the gap
Private Const EDGE_ROW_OFFSET   As Double = 1.5   ' half the vertical gap between the two lines of a double edge bar

#If VBA7 Then
    Private Const INVALID_HANDLE As LongPtr = -1
#Else
    Private Const INVALID_HANDLE As Long = -1
#End If

'' == Windows API =============================================================

#If VBA7 Then
    Private Declare PtrSafe Function CreateFile Lib "kernel32" Alias "CreateFileW" ( _
        ByVal lpFileName       As LongPtr, _
        ByVal dwDesiredAccess  As Long, _
        ByVal dwShareMode      As Long, _
        ByVal lpSecurityAttrib As LongPtr, _
        ByVal dwCreationDisp   As Long, _
        ByVal dwFlagsAndAttrib As Long, _
        ByVal hTemplateFile    As LongPtr _
    ) As LongPtr
    Private Declare PtrSafe Function ReadFile Lib "kernel32" ( _
        ByVal hFile                As LongPtr, _
        ByVal lpBuffer             As LongPtr, _
        ByVal nNumberOfBytesToRead As Long, _
        ByRef lpNumberOfBytesRead  As Long, _
        ByVal lpOverlapped         As LongPtr _
    ) As Long
    Private Declare PtrSafe Function CloseHandle Lib "kernel32" ( _
        ByVal hObject As LongPtr _
    ) As Long
    Private Declare PtrSafe Function WaitNamedPipe Lib "kernel32" Alias "WaitNamedPipeW" ( _
        ByVal lpNamedPipeName As LongPtr, _
        ByVal nTimeOut        As Long _
    ) As Long
    Private Declare PtrSafe Sub Sleep Lib "kernel32" ( _
        ByVal dwMilliseconds As Long _
    )
#Else
    Private Declare Function CreateFile Lib "kernel32" Alias "CreateFileW" ( _
        ByVal lpFileName       As Long, _
        ByVal dwDesiredAccess  As Long, _
        ByVal dwShareMode      As Long, _
        ByVal lpSecurityAttrib As Long, _
        ByVal dwCreationDisp   As Long, _
        ByVal dwFlagsAndAttrib As Long, _
        ByVal hTemplateFile    As Long _
    ) As Long
    Private Declare Function ReadFile Lib "kernel32" ( _
        ByVal hFile                As Long, _
        ByVal lpBuffer             As Long, _
        ByVal nNumberOfBytesToRead As Long, _
        ByRef lpNumberOfBytesRead  As Long, _
        ByVal lpOverlapped         As Long _
    ) As Long
    Private Declare Function CloseHandle Lib "kernel32" ( _
        ByVal hObject As Long _
    ) As Long
    Private Declare Function WaitNamedPipe Lib "kernel32" Alias "WaitNamedPipeW" ( _
        ByVal lpNamedPipeName As Long, _
        ByVal nTimeOut        As Long _
    ) As Long
    Private Declare Sub Sleep Lib "kernel32" ( _
        ByVal dwMilliseconds As Long _
    )
#End If

'' == State ====================================================================

Private g_Running As Boolean

'' == Helpers ==================================================================

' Extract sheet name -- a Function, not a Const, because VBA Consts cannot
' call RuStr() (compile-time constant expressions only). Kept as the very
' first procedure in the module (all Const/Declare/Dim must precede every
' Sub/Function/Property) so it stays as close as possible to the EXTR_
' constants above it.
Private Function ExtrSheetName() As String
    ExtrSheetName = RuStr(&H412, &H44B, &H43F, &H438, &H441, &H43A, &H430)  ' "Выписка"
End Function

' Escapes a string for JSON: non-ASCII and control chars become \uXXXX.
Private Function JsonEscapeStr(s As String) As String
    Dim result As String
    Dim i As Integer
    Dim c As Long
    result = ""
    For i = 1 To Len(s)
        c = AscW(Mid(s, i, 1))
        If c < 0 Then c = c + 65536
        Select Case c
            Case 34:      result = result & "\"""
            Case 92:      result = result & "\\"
            Case Is < 32: result = result & "\u" & Right("0000" & Hex(c), 4)
            Case Is > 127:result = result & "\u" & Right("0000" & Hex(c), 4)
            Case Else:    result = result & Chr(c)
        End Select
    Next i
    JsonEscapeStr = result
End Function

#If VBA7 Then
Private Function WStrPtr(s As String) As LongPtr
    WStrPtr = StrPtr(s)
End Function
#Else
Private Function WStrPtr(s As String) As Long
    WStrPtr = StrPtr(s)
End Function
#End If

' Decodes a slice of a byte array from UTF-8 to a VBA Unicode string.
Private Function Utf8ToStr(buf() As Byte, nBytes As Long) As String
    If nBytes = 0 Then Utf8ToStr = "": Exit Function
    Dim tmp() As Byte
    ReDim tmp(nBytes - 1)
    Dim i As Long
    For i = 0 To nBytes - 1
        tmp(i) = buf(i)
    Next i
    With CreateObject("ADODB.Stream")
        .Open
        .Type = 1        ' adTypeBinary
        .Write tmp
        .Position = 0
        .Type = 2        ' adTypeText
        .Charset = "UTF-8"
        Utf8ToStr = .ReadText
        .Close
    End With
End Function

Private Function DataCol(ws As Worksheet) As Long
    DataCol = ws.Range(DATA_CELL).Column
End Function

Private Function DataHeaderRow(ws As Worksheet) As Long
    DataHeaderRow = ws.Range(DATA_CELL).Row
End Function

Private Function DataStartRow(ws As Worksheet) As Long
    DataStartRow = ws.Range(DATA_CELL).Row + 1
End Function

Private Function DataEndRow(ws As Worksheet) As Long
    DataEndRow = ws.Range(DATA_CELL).Row + MAX_PIECE_ROWS
End Function

' A row is considered empty if its height or width equals zero
Private Function IsBlankPieceRow(ws As Worksheet, row As Long, dc As Long) As Boolean
    Dim w As Long, h As Long
    w = 0: h = 0
    If ws.Cells(row, dc + 1).Value <> "" Then w = CLng(ws.Cells(row, dc + 1).Value)
    If ws.Cells(row, dc + 2).Value <> "" Then h = CLng(ws.Cells(row, dc + 2).Value)
    IsBlankPieceRow = (w = 0 Or h = 0)
End Function

' Final row of the fragment table: individual empty rows act as visual gaps,
' whereas a consecutive pair of blank rows terminates the table (or hits DataEndRow first).
' Returns DataStartRow(ws) - 1 for an empty table.
Private Function LastPieceRow(ws As Worksheet) As Long
    Dim dc As Long: dc = DataCol(ws)
    Dim i As Long, blankRun As Long
    LastPieceRow = DataStartRow(ws) - 1
    blankRun = 0
    For i = DataStartRow(ws) To DataEndRow(ws)
        If IsBlankPieceRow(ws, i, dc) Then
            blankRun = blankRun + 1
            If blankRun >= 2 Then Exit For
        Else
            blankRun = 0
            LastPieceRow = i
        End If
    Next i
End Function

' Writes progress values to status cells.
Private Sub SetProgress(ws As Worksheet, status As String, gen As String, _
                        obj As String, sheets As String)
    ws.Range(OUT_STATUS_CELL).Value = status
    If gen    <> "" Then ws.Range(OUT_GEN_CELL).Value    = CLng(gen)
    If obj    <> "" Then ws.Range(OUT_OBJ_CELL).Value    = CDbl(obj)
    If sheets <> "" Then ws.Range(OUT_SHEETS_CELL).Value = CLng(sheets)
    DoEvents
End Sub

' Writes progress labels (column to the left of status cells) and clears previous results.
Private Sub InitOutputArea(ws As Worksheet)
    Dim rBase As Range: Set rBase = ws.Range(RESULT_CELL)
    ws.Range(ws.Cells(rBase.Row + 1, rBase.Column), ws.Cells(1000, rBase.Column + 6)).ClearContents
    ws.Range(OUT_OBJ_CELL).NumberFormat = "# ### ### ##0"
    ClearCutLengths ws
    ClearLayoutShapes ws
    ClearPieceColors ws
End Sub

' Renders the placement table after a Done message.
Private Sub RenderPlacements(ws As Worksheet, sol As Object, pieces As Object)
    Dim rRow As Long: rRow = ws.Range(RESULT_CELL).Row
    Dim rCol As Long: rCol = ws.Range(RESULT_CELL).Column
    Dim r    As Long: r    = rRow + 1

    Dim pls As Object: Set pls = sol("placements")
    Dim n   As Long:   n   = pls.Count
    If n = 0 Then Exit Sub

    ' Build index array and sort by (sheet_idx, x, y) — bubble sort
    Dim ord() As Long
    ReDim ord(1 To n)
    Dim i As Long, j As Long, tmp As Long
    For i = 1 To n
        ord(i) = i
    Next i
    Dim pa As Object, pb As Object, cmp As Long
    For i = 1 To n - 1
        For j = 1 To n - i
            Set pa = pls(ord(j))
            Set pb = pls(ord(j + 1))
            cmp = CLng(pa("sheet_idx")) - CLng(pb("sheet_idx"))
            If cmp = 0 Then cmp = CLng(pa("x")) - CLng(pb("x"))
            If cmp = 0 Then cmp = CLng(pa("y")) - CLng(pb("y"))
            If cmp > 0 Then
                tmp = ord(j): ord(j) = ord(j + 1): ord(j + 1) = tmp
            End If
        Next j
    Next i

    Dim pl As Object, idx As Long, pieceName As String, pw As Long, ph As Long
    For i = 1 To n
        Set pl = pls(ord(i))
        idx = pl("ptype_idx") + 1
        pieceName = pieces(idx)("name")
        If pl("rotated") Then
            pw = pieces(idx)("height"): ph = pieces(idx)("width")
        Else
            pw = pieces(idx)("width"):  ph = pieces(idx)("height")
        End If
        ws.Cells(r, rCol).Value     = pl("sheet_idx") + 1
        ws.Cells(r, rCol + 1).Value = pieceName
        ws.Cells(r, rCol + 2).Value = pl("x")
        ws.Cells(r, rCol + 3).Value = pl("y")
        ws.Cells(r, rCol + 4).Value = pw
        ws.Cells(r, rCol + 5).Value = ph
        ws.Cells(r, rCol + 6).Value = IIf(pl("rotated"), "yes", "")
        r = r + 1
    Next i
    DoEvents
End Sub

'' == Layout drawing ===========================================================

' Returns a deterministic fill color from a 12-color palette by row index (0-based).
Private Function PieceColor(ByVal colorIdx As Long) As Long
    Dim p(11) As Long
    p(0)  = RGB(255, 182, 193): p(1)  = RGB(173, 216, 230)
    p(2)  = RGB(144, 238, 144): p(3)  = RGB(255, 255, 153)
    p(4)  = RGB(255, 200, 120): p(5)  = RGB(221, 160, 221)
    p(6)  = RGB(135, 206, 235): p(7)  = RGB(240, 180, 180)
    p(8)  = RGB(180, 255, 180): p(9)  = RGB(255, 228, 196)
    p(10) = RGB(200, 200, 255): p(11) = RGB(255, 240, 180)
    PieceColor = p(colorIdx Mod 12)
End Function

' Deletes all shapes whose name starts with "cut_".
Private Sub ClearLayoutShapes(ws As Worksheet)
    Dim shapeNames() As String
    Dim n As Long: n = 0
    Dim shp As Shape
    For Each shp In ws.Shapes
        If Left(shp.Name, 4) = "cut_" Then
            ReDim Preserve shapeNames(n)
            shapeNames(n) = shp.Name
            n = n + 1
        End If
    Next shp
    Dim i As Long
    For i = 0 To n - 1
        ws.Shapes(shapeNames(i)).Delete
    Next i
End Sub

' Draws cutting layout as Excel shapes; origin and width taken from CANVAS_RANGE.
' Sheets are rotated 90° CCW (solver-Y → display-X, solver-X → display-Y) to match
' the AutoCAD drawing orientation, and arranged in two columns within CANVAS_RANGE.
' Pieces are color-coded by piece type.
Private Sub DrawLayout(ws As Worksheet, sol As Object, pieces As Object, _
                       shW As Long, shH As Long)
    If shW <= 0 Or shH <= 0 Then Exit Sub

    ClearLayoutShapes ws

    Dim cvs        As Range:  Set cvs     = ws.Range(CANVAS_RANGE)
    Dim originLeft As Double: originLeft  = cvs.Cells(1, 1).Left
    Dim originTop  As Double: originTop   = cvs.Cells(1, 1).Top
    Dim canvasW    As Double: canvasW     = cvs.Width

    ' After 90° CCW rotation the sheet is shH wide and shW tall in display space.
    ' Two columns fit side by side within canvasW.
    Dim colW    As Double: colW    = (canvasW - CANVAS_SHEET_GAP) / 2
    Dim scl     As Double: scl     = colW / shH
    Dim shDispW As Double: shDispW = shH * scl   ' = colW
    Dim shDispH As Double: shDispH = shW * scl

    ' Count sheets
    Dim nSheets As Long: nSheets = 0
    Dim pl As Object
    For Each pl In sol("placements")
        If pl("sheet_idx") + 1 > nSheets Then nSheets = pl("sheet_idx") + 1
    Next pl

    ' Draw sheet backgrounds in 2-column grid
    Dim si As Long
    For si = 0 To nSheets - 1
        Dim bgLeft As Double: bgLeft = originLeft + (si Mod 2) * (shDispW + CANVAS_SHEET_GAP)
        Dim bgTop  As Double: bgTop  = originTop  + (si \ 2)   * (shDispH + CANVAS_SHEET_GAP)
        Dim bg As Shape
        Set bg = ws.Shapes.AddShape(msoShapeRectangle, bgLeft, bgTop, shDispW, shDispH)
        bg.Name = "cut_bg_" & si
        bg.Fill.ForeColor.RGB = RGB(248, 248, 248)
        bg.Fill.Transparency = 0
        bg.Line.ForeColor.RGB = RGB(60, 60, 60)
        bg.Line.Weight = 1#
        bg.TextFrame.Characters.Text = "Sheet " & si
        bg.TextFrame.Characters.Font.Size = 8
    Next si

    ' Draw pieces (rotated 90° CCW: display_x = solver_y, display_y = solver_x)
    Dim pIdx As Long: pIdx = 0
    For Each pl In sol("placements")
        Dim idx   As Long: idx   = pl("ptype_idx") + 1
        Dim shIdx As Long: shIdx = pl("sheet_idx")

        Dim pw As Long, ph As Long
        If pl("rotated") Then
            pw = pieces(idx)("height"): ph = pieces(idx)("width")
        Else
            pw = pieces(idx)("width"):  ph = pieces(idx)("height")
        End If

        Dim shLeft  As Double: shLeft  = originLeft + (shIdx Mod 2) * (shDispW + CANVAS_SHEET_GAP)
        Dim shTop   As Double: shTop   = originTop  + (shIdx \ 2)   * (shDispH + CANVAS_SHEET_GAP)
        Dim rLeft   As Double: rLeft   = shLeft + pl("y") * scl
        Dim rTop    As Double: rTop    = shTop  + pl("x") * scl
        Dim rWidth  As Double: rWidth  = ph * scl
        Dim rHeight As Double: rHeight = pw * scl
        If rWidth  < 1# Then rWidth  = 1#
        If rHeight < 1# Then rHeight = 1#

        Dim s As Shape
        Set s = ws.Shapes.AddShape(msoShapeRectangle, rLeft, rTop, rWidth, rHeight)
        s.Name = "cut_p_" & pIdx
        pIdx = pIdx + 1
        s.Fill.ForeColor.RGB = PieceColor(idx - 1)
        s.Fill.Transparency = 0
        s.Line.ForeColor.RGB = RGB(80, 80, 80)
        s.Line.Weight = 0.5

        If rWidth >= 20# And rHeight >= 12# Then
            s.TextFrame.Characters.Text = pieces(idx)("name")
            s.TextFrame.Characters.Font.Size = 7
            s.TextFrame.Characters.Font.Color = vbBlack
            s.TextFrame.HorizontalAlignment = xlHAlignCenter
            s.TextFrame.VerticalAlignment = xlVAlignCenter
        Else
            s.TextFrame.Characters.Text = ""
        End If
    Next pl
End Sub

'' == Piece row coloring =======================================================

Private Sub ClearPieceColors(ws As Worksheet)
    ws.Range(ws.Cells(DataStartRow(ws), DataCol(ws)), ws.Cells(DataEndRow(ws), DataCol(ws) + 4)).Interior.ColorIndex = xlNone
End Sub

'' == Statement ("Выписка") ====================================================

' Builds a string from Unicode code points (ParamArray of Long). Used for
' Cyrillic literals so this source file stays pure ASCII -- copy/paste into
' the Excel 2007 VBA editor (ANSI-only) otherwise mangles non-ASCII text
' (UTF-8 bytes get reinterpreted as cp1251, e.g. "Выписка" -> "Р’С‹РїРёСЃРєР°").
Private Function RuStr(ParamArray codes() As Variant) As String
    Dim s As String, i As Long
    For i = LBound(codes) To UBound(codes)
        s = s & ChrW(codes(i))
    Next i
    RuStr = s
End Function

Private Function ExtrLeftCol(ws As Worksheet) As Long
    ExtrLeftCol = ws.Range(EXTR_LEFT_CELL).Column
End Function

Private Function ExtrRightCol(ws As Worksheet) As Long
    ExtrRightCol = ws.Range(EXTR_RIGHT_CELL).Column
End Function

Private Function ExtrHeaderRow(ws As Worksheet) As Long
    ExtrHeaderRow = ws.Range(EXTR_LEFT_CELL).Row
End Function

Private Function ExtrDataRow(ws As Worksheet) As Long
    ExtrDataRow = ExtrHeaderRow(ws) + 1
End Function

' Returns the sheet at EXTR_SHEET_INDEX, creating a blank one right after
' wsIn if the workbook doesn't have that many tabs yet. Formatting (column
' widths, fonts, borders, header text) is not set up here -- the sheet
' itself is the template, edited by hand in Excel.
Private Function GetOrCreateExtractSheet(wsIn As Worksheet) As Worksheet
    Dim ws As Worksheet
    If ThisWorkbook.Sheets.Count >= EXTR_SHEET_INDEX Then
        Set ws = ThisWorkbook.Sheets(EXTR_SHEET_INDEX)
    Else
        Set ws = ThisWorkbook.Sheets.Add(After:=wsIn)
        ws.Name = ExtrSheetName()
    End If
    Set GetOrCreateExtractSheet = ws
End Function

' Fills in the "Extract" template sheet from the current piece table: the
' sheet count in the A1 title, the two-block table of pieces (Панель, N п/п,
' Длина, Ширина, Кол-во), the per-piece edge-banding bars, and the per-pair
' grid border. Static formatting (column widths, fonts, header text, etc.) is
' owned by the template and is never modified here. Re-running is idempotent:
' the same sheet is reused and the Примечание column is left untouched.
' Switches the active sheet to the extract sheet when done.
Public Sub PrepareExtract()
    Dim wsIn As Worksheet
    Set wsIn = ThisWorkbook.Sheets(MAIN_SHEET_INDEX)

    Dim wsOut As Worksheet
    Set wsOut = GetOrCreateExtractSheet(wsIn)

    UpdateExtractTitle wsOut, wsIn

    Dim names() As String, widths() As Long, heights() As Long
    Dim cnts() As Long, ews() As Long, ehs() As Long, ets() As Long
    Dim n As Long
    n = ReadPieceRows(wsIn, names, widths, heights, cnts, ews, ehs, ets)

    Dim half As Long: half = -Int(-n / 2)  ' ceil(n / 2)

    ' Each piece takes a (data row, bar row) pair; find how many such pairs
    ' the sheet currently has, so a shrinking list also clears its old tail.
    Dim leftCol As Long, rightCol As Long, dataRow0 As Long
    leftCol = ExtrLeftCol(wsOut)
    rightCol = ExtrRightCol(wsOut)
    dataRow0 = ExtrDataRow(wsOut)

    Dim lastL As Long, lastR As Long, prevPairsL As Long, prevPairsR As Long
    lastL = wsOut.Cells(wsOut.Rows.Count, leftCol + 1).End(xlUp).Row
    lastR = wsOut.Cells(wsOut.Rows.Count, rightCol + 1).End(xlUp).Row
    prevPairsL = 0: If lastL >= dataRow0 Then prevPairsL = (lastL - dataRow0) \ 2 + 1
    prevPairsR = 0: If lastR >= dataRow0 Then prevPairsR = (lastR - dataRow0) \ 2 + 1

    Dim maxPairs As Long: maxPairs = half
    If prevPairsL > maxPairs Then maxPairs = prevPairsL
    If prevPairsR > maxPairs Then maxPairs = prevPairsR

    ClearExtractBlock wsOut, leftCol, dataRow0, maxPairs
    ClearExtractBlock wsOut, rightCol, dataRow0, maxPairs
    ClearShapesByPrefix wsOut, EXTR_SHAPE_PREFIX

    Dim r As Long, leftIdx As Long, rightIdx As Long, dataRow As Long
    For r = 1 To half
        dataRow = dataRow0 + (r - 1) * 2
        leftIdx = r
        rightIdx = r + half
        If leftIdx <= n Then
            FillExtractRow wsOut, leftCol, dataRow, leftIdx, _
                names(leftIdx), widths(leftIdx), heights(leftIdx), cnts(leftIdx), ews(leftIdx), ehs(leftIdx), ets(leftIdx)
        End If
        If rightIdx <= n Then
            FillExtractRow wsOut, rightCol, dataRow, rightIdx, _
                names(rightIdx), widths(rightIdx), heights(rightIdx), cnts(rightIdx), ews(rightIdx), ehs(rightIdx), ets(rightIdx)
        End If
    Next r

    wsOut.Activate
End Sub

' Reads the piece table from Sheet1 (same rows/columns as TotalEdgeLength /
' BuildProblemJson) into parallel 1-based arrays. Returns the row count.
Private Function ReadPieceRows(ws As Worksheet, ByRef names() As String, ByRef widths() As Long, _
                                ByRef heights() As Long, ByRef cnts() As Long, _
                                ByRef ews() As Long, ByRef ehs() As Long, _
                                ByRef ets() As Long) As Long
    ReDim names(1 To MAX_PIECE_ROWS)
    ReDim widths(1 To MAX_PIECE_ROWS)
    ReDim heights(1 To MAX_PIECE_ROWS)
    ReDim cnts(1 To MAX_PIECE_ROWS)
    ReDim ews(1 To MAX_PIECE_ROWS)
    ReDim ehs(1 To MAX_PIECE_ROWS)
    ReDim ets(1 To MAX_PIECE_ROWS)

    Dim dc As Long: dc = DataCol(ws)
    Dim lastRow As Long: lastRow = LastPieceRow(ws)
    Dim n As Long: n = 0
    Dim i As Long
    For i = DataStartRow(ws) To lastRow
        If Not IsBlankPieceRow(ws, i, dc) Then
            Dim w As Long, h As Long
            w = 0: h = 0
            If ws.Cells(i, dc + 1).Value <> "" Then w = CLng(ws.Cells(i, dc + 1).Value)
            If ws.Cells(i, dc + 2).Value <> "" Then h = CLng(ws.Cells(i, dc + 2).Value)

            n = n + 1
            names(n) = Trim(ws.Cells(i, dc).Value)
            widths(n) = w
            heights(n) = h
            ' dc + 3 = "D" is the counts column
            cnts(n) = 0
            If ws.Cells(i, dc + 3).Value <> "" Then cnts(n) = CLng(ws.Cells(i, dc + 3).Value)
            ' dc + 5 = "F" is the edge widths column
            ews(n) = 0
            If ws.Cells(i, dc + 5).Value <> "" Then ews(n) = CLng(ws.Cells(i, dc + 5).Value)
            ' dc + 6 = "G" is the edge heights column
            ehs(n) = 0
            If ws.Cells(i, dc + 6).Value <> "" Then ehs(n) = CLng(ws.Cells(i, dc + 6).Value)
            ' dc + 7 = "H" is the edge types column
            ets(n) = 0
            If ws.Cells(i, dc + 7).Value <> "" Then ets(n) = CLng(ws.Cells(i, dc + 7).Value)
        End If
    Next i

    ReadPieceRows = n
End Function

' Clears the Панель..Кол-во columns of one block's `maxPairs` (data row, bar
' row) pairs (leaves the Примечание column and the header row untouched), and
' resets the per-pair grid borders over the full block width (so unused pairs
' from a shrinking list lose their border too; FillExtractRow reapplies it
' for pairs that are still in use). The edge-bar line shapes are cleared
' separately via ClearShapesByPrefix.
Private Sub ClearExtractBlock(ws As Worksheet, col As Long, dataRow0 As Long, maxPairs As Long)
    If maxPairs <= 0 Then Exit Sub
    Dim lastRow As Long: lastRow = dataRow0 + maxPairs * 2 - 1
    ws.Range(ws.Cells(dataRow0, col), ws.Cells(lastRow, col + EXTR_COLS - 2)).ClearContents
    ws.Range(ws.Cells(dataRow0, col), ws.Cells(lastRow, col + EXTR_COLS - 1)).Borders.LineStyle = xlNone
End Sub

' Updates the sheet count in the A1 title, preserving any user-written
' description text. If A1 is empty, writes "(N листов)."; if it already ends
' with a "(<count> <word starting with "лист">)[.]" pattern, only the count
' is replaced; otherwise "(N листов)." is appended to the existing text.
Private Sub UpdateExtractTitle(ws As Worksheet, wsIn As Worksheet)
    Dim sheetsUsed As Variant: sheetsUsed = wsIn.Range(OUT_SHEETS_CELL).Value

    Dim cell As Range: Set cell = ws.Range("A1")
    Dim cur As String: cur = CStr(cell.Value)
    Dim root As String: root = RuStr(&H43B, &H438, &H441, &H442)            ' "лист"
    Dim wordListov As String: wordListov = root & RuStr(&H43E, &H432)       ' "листов"

    Dim re As Object
    Set re = CreateObject("VBScript.RegExp")
    re.Pattern = "\(\d+(\s+" & root & "[^)]*)\)(\.?)\s*$"

    If Trim(cur) = "" Then
        cell.Value = "(" & CStr(sheetsUsed) & " " & wordListov & ")."
    ElseIf re.Test(cur) Then
        cell.Value = re.Replace(cur, "(" & CStr(sheetsUsed) & "$1)$2")
    Else
        cell.Value = cur & " (" & CStr(sheetsUsed) & " " & wordListov & ")."
    End If
End Sub

' Writes the data row of one piece (row = dataRow) and the edge-banding
' border row directly below it (row = dataRow + 1).
Private Sub FillExtractRow(ws As Worksheet, col As Long, dataRow As Long, idx As Long, _
                              pieceName As String, w As Long, h As Long, cnt As Long, _
                              ew As Long, eh As Long, edgeType As Long)
    ws.Cells(dataRow, col).Value = pieceName  ' template column is pre-formatted as text
    ws.Cells(dataRow, col + 1).Value = idx
    ws.Cells(dataRow, col + 2).Value = w
    ws.Cells(dataRow, col + 3).Value = h
    ws.Cells(dataRow, col + 4).Value = cnt

    Dim barRow As Long: barRow = dataRow + 1
    DrawEdgeBar ws, ws.Cells(barRow, col + 2), ew, edgeType, EXTR_SHAPE_PREFIX & barRow & "_" & (col + 2)
    DrawEdgeBar ws, ws.Cells(barRow, col + 3), eh, edgeType, EXTR_SHAPE_PREFIX & barRow & "_" & (col + 3)

    SetThinOutlineBorder ws.Range(ws.Cells(dataRow, col), ws.Cells(barRow, col + EXTR_COLS - 1))
End Sub

' Draws a horizontal line shape across the vertical middle of cellRng (equal
' margins above and below, and a small inset from the left/right cell edges)
' representing the edge-banding count. Both edgeType 0 (solid, DrawSolidEdgeBar)
' and edgeType 1 (dashed, DrawDashedEdgeBar) are built from plain AddLine
' shapes rather than Shape.Line's compound Style/DashStyle, which does not
' combine reliably for a double line in Excel.
Private Sub DrawEdgeBar(ws As Worksheet, cellRng As Range, count As Long, edgeType As Long, shapeName As String)
    If count <= 0 Then Exit Sub

    Dim inset As Double: inset = 5
    Dim xLeft  As Double: xLeft  = cellRng.Left + inset
    Dim xRight As Double: xRight = cellRng.Left + cellRng.Width - inset
    Dim yMid   As Double: yMid   = cellRng.Top + cellRng.Height / 2

    If edgeType = 1 Then
        DrawDashedEdgeBar ws, xLeft, xRight, yMid, count, shapeName
    Else
        DrawSolidEdgeBar ws, xLeft, xRight, yMid, count, shapeName
    End If
End Sub

' Draws an edgeType=0 bar as one solid line (count=1) or two parallel solid
' lines grouped into a single Shape (count=2), so the double edge is one
' object -- selectable/deletable as a unit -- exactly like the dashed bar.
Private Sub DrawSolidEdgeBar(ws As Worksheet, xLeft As Double, xRight As Double, _
                              yMid As Double, count As Long, shapeName As String)
    If count = 1 Then
        Dim ln As Shape
        Set ln = ws.Shapes.AddLine(xLeft, yMid, xRight, yMid)
        StyleSolidLine ln, shapeName
        Exit Sub
    End If

    Dim ln1 As Shape, ln2 As Shape
    Set ln1 = ws.Shapes.AddLine(xLeft, yMid - EDGE_ROW_OFFSET, xRight, yMid - EDGE_ROW_OFFSET)
    Set ln2 = ws.Shapes.AddLine(xLeft, yMid + EDGE_ROW_OFFSET, xRight, yMid + EDGE_ROW_OFFSET)
    StyleSolidLine ln1, ln1.Name
    StyleSolidLine ln2, ln2.Name

    Dim grp As Shape
    Set grp = ws.Shapes.Range(Array(ln1.Name, ln2.Name)).Group
    grp.Name = shapeName
End Sub

Private Sub StyleSolidLine(ln As Shape, nm As String)
    ln.Name = nm
    ln.Placement = xlMove  ' don't let column/row resizes rescale the inset
    ln.Line.ForeColor.RGB = RGB(0, 0, 0)
    ln.Line.Weight = 1
    ln.Line.Style = msoLineSingle
    ln.Line.DashStyle = msoLineSolid
End Sub

' Draws an edgeType=1 bar as explicit short line-segment shapes instead of
' via Shape.Line.DashStyle: a ThinThin (double) line combined with a dash
' style silently collapses to a plain dotted single line in Excel, so the
' dashed look is built by hand instead. count=1 draws one row of dashes;
' count=2 draws two rows offset above/below the midline, giving the "="
' appearance of a double dashed edge. Always draws exactly EDGE_DASH_COUNT
' dashes per row, with EDGE_DASH_COUNT-1 gaps *between* them (not after the
' last one), so the row spans the full xLeft..xRight width -- same as a
' solid bar -- instead of falling short by one trailing gap.
Private Sub DrawDashedEdgeBar(ws As Worksheet, xLeft As Double, xRight As Double, _
                               yMid As Double, count As Long, shapeName As String)
    Dim gapToDashRatio As Double: gapToDashRatio = EDGE_DASH_GAP_RATIO / (1 - EDGE_DASH_GAP_RATIO)
    Dim dashLen As Double: dashLen = (xRight - xLeft) / (EDGE_DASH_COUNT + (EDGE_DASH_COUNT - 1) * gapToDashRatio)
    Dim gapLen  As Double: gapLen  = dashLen * gapToDashRatio
    Dim stepLen As Double: stepLen = dashLen + gapLen

    Dim rows As Variant
    If count = 1 Then
        rows = Array(yMid)
    Else
        rows = Array(yMid - EDGE_ROW_OFFSET, yMid + EDGE_ROW_OFFSET)
    End If

    Dim r As Long, segIdx As Long: segIdx = 0
    For r = LBound(rows) To UBound(rows)
        Dim i As Long
        For i = 0 To EDGE_DASH_COUNT - 1
            Dim x  As Double: x  = xLeft + i * stepLen
            Dim x2 As Double: x2 = x + dashLen
            Dim seg As Shape
            Set seg = ws.Shapes.AddLine(x, rows(r), x2, rows(r))
            seg.Name = shapeName & "_" & segIdx
            seg.Placement = xlMove
            seg.Line.ForeColor.RGB = RGB(0, 0, 0)
            seg.Line.Weight = 1
            seg.Line.Style = msoLineSingle
            seg.Line.DashStyle = msoLineSolid
            segIdx = segIdx + 1
        Next i
    Next r
End Sub

' Removes all shapes on `ws` whose name starts with `prefix` (the
' previously-drawn edge-banding bars), so PrepareExtract can redraw them
' from scratch on every run.
Private Sub ClearShapesByPrefix(ws As Worksheet, prefix As String)
    Dim i As Long
    For i = ws.Shapes.Count To 1 Step -1
        If Left(ws.Shapes(i).Name, Len(prefix)) = prefix Then ws.Shapes(i).Delete
    Next i
End Sub

' Applies a thin border around the outer edge of `rng` only, with no internal
' lines (e.g. a piece's data+bar row pair, A3:F4 / A5:F6 / ...).
Private Sub SetThinOutlineBorder(rng As Range)
    Dim idx As Variant
    For Each idx In Array(xlEdgeLeft, xlEdgeTop, xlEdgeRight, xlEdgeBottom)
        With rng.Borders(idx)
            .LineStyle = xlContinuous
            .Weight = xlThin
        End With
    Next idx
End Sub

Private Sub ColorPieceRows(ws As Worksheet, pieces As Object)
    Dim dc As Long: dc = DataCol(ws)
    Dim ci As Long: ci = 0
    Dim i As Long
    For i = DataStartRow(ws) To 200
        If ws.Cells(i, dc + 1).Value = "" Then Exit For
        ' ws.Range(ws.Cells(i, dc), ws.Cells(i, dc + 4)).Interior.Color = PieceColor(ci)
        ci = ci + 1
    Next i
End Sub

'' == Cut length ==============================================================

Private Sub ClearCutLengths(ws As Worksheet)
    Dim base As Range: Set base = ws.Range(OUT_CUT_CELL)
    ws.Range(base, ws.Cells(base.Row, base.Column + 50)).ClearContents
End Sub

' Writes pre-computed per-sheet cut lengths from the Rust "done" message.
' cutLengths is msg("cut_lengths") — a JSON array with one value per sheet.
' Values are written to R5, S5, T5, … (OUT_CUT_CELL and to the right).
Private Sub WriteCutLengths(ws As Worksheet, cutLengths As Object)
    If cutLengths Is Nothing Then Exit Sub
    Dim base As Range: Set base = ws.Range(OUT_CUT_CELL)
    Dim si As Long: si = 0
    Dim v As Variant
    For Each v In cutLengths
        With ws.Cells(base.Row, base.Column + si)
            .Value = CLng(v)
            .NumberFormat = "# ### ##0"
        End With
        si = si + 1
    Next v
End Sub

'' == JSON builder =============================================================

Private Function BuildProblemJson(ws As Worksheet) As String
    Dim sheetWidth  As Long: sheetWidth  = ws.Range(SHEET_W_CELL).Value
    Dim sheetHeight As Long: sheetHeight = ws.Range(SHEET_H_CELL).Value
    Dim kerf        As Long: kerf        = ws.Range(KERF_CELL).Value
    Dim margin      As Long: margin      = ws.Range(MARGIN_CELL).Value

    Dim dc As Long: dc = DataCol(ws)
    Dim lastRow As Long: lastRow = LastPieceRow(ws)
    Dim sPieces As String
    Dim bFirst  As Boolean: bFirst = True
    Dim i As Long

    For i = DataStartRow(ws) To lastRow
        If Not IsBlankPieceRow(ws, i, dc) Then
            Dim w As Long, h As Long
            w = 0: h = 0
            If ws.Cells(i, dc + 1).Value <> "" Then w = CLng(ws.Cells(i, dc + 1).Value)
            If ws.Cells(i, dc + 2).Value <> "" Then h = CLng(ws.Cells(i, dc + 2).Value)

            Dim pName   As String:  pName   = Trim(ws.Cells(i, dc).Value)
            Dim pCount  As Long:    pCount  = CLng(ws.Cells(i, dc + 3).Value)
            Dim pRotate As Boolean: pRotate = (ws.Cells(i, dc + 4).Value = True)

            Dim sPiece As String
            sPiece = "{""name"":"""  & JsonEscapeStr(pName) & """" & _
                     ",""width"":"  & CStr(w) & _
                     ",""height"":" & CStr(h) & _
                     ",""count"":"  & CStr(pCount) & _
                     ",""can_rotate"":" & IIf(pRotate, "true", "false") & "}"

            If bFirst Then
                sPieces = sPiece
                bFirst = False
            Else
                sPieces = sPieces & "," & sPiece
            End If
        End If
    Next i

    BuildProblemJson = "{""sheet"":{""width"":" & CStr(sheetWidth) & _
                       ",""height"":" & CStr(sheetHeight) & "}" & _
                       ",""kerf"":" & CStr(kerf) & _
                       ",""margin"":" & CStr(margin) & _
                       ",""piece_types"":[" & sPieces & "]}"
End Function

'' == Main macro ===============================================================

Public Sub RunCut()
    If g_Running Then
        MsgBox "Solver is already running!", vbInformation
        Exit Sub
    End If

    Dim ws As Worksheet
    Set ws = ThisWorkbook.Sheets(MAIN_SHEET_INDEX)

    Dim exePath As String
    exePath = Trim(ws.Cells(1, 2).Value)  ' B1
    If Dir(exePath) = "" Then
        MsgBox "solver.exe not found: " & exePath & Chr(13) & _
               "Set the correct path in cell B1.", vbCritical
        Exit Sub
    End If

    InitOutputArea ws
    SetProgress ws, "Preparing...", "", "", ""

    ' Build JSON and write to a temp file
    Dim jsonStr As String
    jsonStr = BuildProblemJson(ws)

    Dim tmpFile As String
    tmpFile = Environ("TEMP") & "\cut_input.json"

    Dim fNum As Integer
    fNum = FreeFile
    Open tmpFile For Output As #fNum
    Print #fNum, jsonStr
    Close #fNum

    ' Read GA config from sheet (use defaults if cells empty)
    Dim nSeed As Long: nSeed = 42
    Dim nGens As Long: nGens = 2000
    Dim nPop  As Long: nPop  = 200
    If ws.CheckBoxes(CFG_RANDOM_SEED_CHK).Value = xlOn Then
        Randomize
        nSeed = Int(Rnd() * 10000)
        ws.Range(CFG_SEED_CELL).Value = nSeed
    ElseIf ws.Range(CFG_SEED_CELL).Value <> "" Then
        nSeed = CLng(ws.Range(CFG_SEED_CELL).Value)
    End If
    If ws.Range(CFG_GENS_CELL).Value <> "" Then nGens = CLng(ws.Range(CFG_GENS_CELL).Value)
    If ws.Range(CFG_POP_CELL).Value  <> "" Then nPop  = CLng(ws.Range(CFG_POP_CELL).Value)
    Dim sAlgorithm As String: sAlgorithm = "glas"
    If ws.Range(CFG_ALGORITHM_CELL).Value <> "" Then _
        sAlgorithm = LCase(Trim(CStr(ws.Range(CFG_ALGORITHM_CELL).Value)))
    ' Dropdown stores "code - Russian description"; keep only the code for the CLI.
    If InStr(sAlgorithm, " - ") > 0 Then sAlgorithm = Trim(Left(sAlgorithm, InStr(sAlgorithm, " - ") - 1))
    Dim nLargeArea As Long: nLargeArea = 0
    If ws.Range(CFG_LARGE_AREA_CELL).Value <> "" Then nLargeArea = CLng(ws.Range(CFG_LARGE_AREA_CELL).Value)
    Dim nLongDim As Long: nLongDim = 0
    If ws.Range(CFG_LONG_DIM_CELL).Value <> "" Then nLongDim = CLng(ws.Range(CFG_LONG_DIM_CELL).Value)

    ' Launch solver.exe (non-blocking Shell); threads = 0 means auto-detect
    Dim cmd As String
    cmd = Chr(34) & exePath & Chr(34) & " calc --json " & Chr(34) & tmpFile & Chr(34) _
        & " --seed " & nSeed & " --iterations " & nGens & " --pop " & nPop _
        & " --algorithm " & sAlgorithm & " --sink pipe"
    If nLargeArea > 0 Then cmd = cmd & " --large-area-threshold " & nLargeArea
    If nLongDim > 0 Then cmd = cmd & " --long-dim-threshold " & nLongDim
    Shell cmd, vbHide

    ' Give solver.exe time to create the pipe
    Sleep 800

    ' Connect to the named pipe (with retries)
    Dim pipeName As String
    pipeName = PIPE_NAME

    #If VBA7 Then
        Dim hPipe As LongPtr
    #Else
        Dim hPipe As Long
    #End If
    Dim attempt As Integer
    For attempt = 1 To 15
        WaitNamedPipe WStrPtr(pipeName & Chr(0)), 2000
        hPipe = CreateFile( _
            WStrPtr(pipeName & Chr(0)), _
            GENERIC_READ, 0, 0, OPEN_EXISTING, FILE_ATTR_NORMAL, 0)
        If hPipe <> INVALID_HANDLE Then Exit For
        Sleep 400
        DoEvents
    Next attempt

    If hPipe = INVALID_HANDLE Then
        MsgBox "Could not connect to named pipe." & Chr(13) & _
               "Make sure solver.exe started successfully.", vbCritical
        SetProgress ws, "Connection error", "", "", ""
        Exit Sub
    End If

    '' == Message read loop ====================================================
    g_Running = True
    SetProgress ws, "Running...", "", "", ""

    Dim buf()    As Byte
    Dim nRead    As Long
    Dim raw      As String
    Dim leftover As String
    ReDim buf(BUFFER_SIZE - 1)
    leftover = ""

    Do While g_Running
        nRead = 0
        Dim ok As Long
        ok = ReadFile(hPipe, VarPtr(buf(0)), BUFFER_SIZE, nRead, 0)

        If ok = 0 Or nRead = 0 Then Exit Do   ' pipe closed

        raw = leftover & Utf8ToStr(buf, nRead)
        leftover = ""

        ' Split into lines
        Dim lines() As String
        lines = Split(raw, Chr(10))

        Dim li As Integer
        For li = 0 To UBound(lines)
            Dim ln As String
            ln = Trim(lines(li))
            If ln = "" Then GoTo NextLine

            ' Last element from Split without trailing \n - incomplete line
            If li = UBound(lines) And Right(raw, 1) <> Chr(10) Then
                leftover = ln
                GoTo NextLine
            End If

            ' Parse JSON line
            On Error Resume Next
            Dim msg As Object
            Set msg = JsonConverter.ParseJson(ln)
            On Error GoTo 0

            If msg Is Nothing Then GoTo NextLine

            Select Case msg("type")
                Case "progress"
                    SetProgress ws, "Running...", _
                        CStr(msg("generation")), _
                        CStr(msg("secondary_objective")), _
                        CStr(msg("sheets_used"))

                Case "done"
                    SetProgress ws, "Done " & ChrW(10003), "", _
                        CStr(msg("secondary_objective")), _
                        CStr(msg("sheets_used"))
                    Application.ScreenUpdating = False
                    Dim rBase As Range: Set rBase = ws.Range(RESULT_CELL)
                    ws.Range(ws.Cells(rBase.Row + 1, rBase.Column), ws.Cells(1000, rBase.Column + 6)).ClearContents
                    RenderPlacements ws, msg("solution"), msg("pieces")
                    DrawLayout ws, msg("solution"), msg("pieces"), _
                        ws.Range(SHEET_W_CELL).Value, ws.Range(SHEET_H_CELL).Value
                    ColorPieceRows ws, msg("pieces")
                    WriteCutLengths ws, msg("cut_lengths")
                    Application.ScreenUpdating = True
                    g_Running = False

                Case "error"
                    SetProgress ws, "Error: " & msg("message"), "", "", ""
                    g_Running = False
            End Select

            Set msg = Nothing
NextLine:
        Next li

        DoEvents
        Sleep 100
    Loop

    CloseHandle hPipe
    g_Running = False
End Sub

Public Function IsRunning() As Boolean
    IsRunning = g_Running
End Function

' Calculates total edge banding length (mm) from edging columns F and G,
' counting only rows whose edge type (column H, 0 default or 1) equals
' edgeType. F = number of width edges (0-2), G = number of height edges (0-2).
' Overhang (припуск) is read from EDGE_MARGIN_CELL (K5); default = 40 mm if empty.
' Usage: put =TotalEdgeLength(0) in S6 and =TotalEdgeLength(1) in S7;
' recalculates on every sheet change.
Public Function TotalEdgeLength(edgeType As Long) As Long
    Application.Volatile
    Dim ws As Worksheet
    Set ws = ThisWorkbook.Sheets(MAIN_SHEET_INDEX)

    Dim overhang As Long
    overhang = CLng(ws.Range(EDGE_MARGIN_CELL).Value)

    Dim dc As Long:    dc    = DataCol(ws)
    Dim lastRow As Long: lastRow = LastPieceRow(ws)
    Dim total As Long: total = 0
    Dim i As Long

    For i = DataStartRow(ws) To lastRow
        If Not IsBlankPieceRow(ws, i, dc) Then
            Dim w As Long, h As Long
            w = 0: h = 0
            If ws.Cells(i, dc + 1).Value <> "" Then w = CLng(ws.Cells(i, dc + 1).Value)
            If ws.Cells(i, dc + 2).Value <> "" Then h = CLng(ws.Cells(i, dc + 2).Value)

            Dim et As Long: et = 0
            If ws.Cells(i, dc + 7).Value <> "" Then et = CLng(ws.Cells(i, dc + 7).Value)
            If et = edgeType Then
                Dim cnt As Long: cnt = 0
                If ws.Cells(i, dc + 3).Value <> "" Then cnt = CLng(ws.Cells(i, dc + 3).Value)

                Dim ew As Long: ew = 0
                Dim eh As Long: eh = 0
                If ws.Cells(i, dc + 5).Value <> "" Then ew = CLng(ws.Cells(i, dc + 5).Value)
                If ws.Cells(i, dc + 6).Value <> "" Then eh = CLng(ws.Cells(i, dc + 6).Value)

                total = total + cnt * ew * (w + overhang)
                total = total + cnt * eh * (h + overhang)
            End If
        End If
    Next i

    TotalEdgeLength = total
End Function

Public Sub RestartCut()
    If g_Running Then
        StopCut
        Application.OnTime Now + TimeValue("00:00:01"), "Cut.RunCut"
    Else
        RunCut
    End If
End Sub

Public Sub StopCut()
    g_Running = False
    Dim ws As Worksheet
    Set ws = ThisWorkbook.Sheets(MAIN_SHEET_INDEX)
    ws.Range(OUT_STATUS_CELL).Value = "Stopped"
    Dim rBase As Range: Set rBase = ws.Range(RESULT_CELL)
    ws.Range(ws.Cells(rBase.Row + 1, rBase.Column), ws.Cells(1000, rBase.Column + 6)).ClearContents
    ClearCutLengths ws
    ClearLayoutShapes ws
    ClearPieceColors ws
End Sub

'' == AutoCAD export ==========================================================

' Returns a running AutoCAD 2015 instance, or Nothing if AutoCAD is not open.
Private Function GetAcadInstance() As Object
    Dim acad As Object
    On Error Resume Next
    Set acad = GetObject(, "AutoCAD.Application.20")
    On Error GoTo 0
    Set GetAcadInstance = acad
End Function

' Adds a single centered text label to blkDef using the drawing's current text style.
' Text height = labelH (mm, same for all pieces); tall-narrow blocks are rotated 90°.
Private Sub AddPieceLabel(blkDef As Object, bw As Long, bh As Long, _
                          pw As Long, ph As Long, pieceName As String, _
                          labelH As Double)
    Dim rotated90 As Boolean: rotated90 = (bw < bh)

    Dim labelDim As String: labelDim = CStr(pw) & "x" & CStr(ph)
    Dim lbl As String
    If Len(pieceName) > 0 Then
        lbl = labelDim & " (" & pieceName & ")"
    Else
        lbl = labelDim
    End If

    Dim txtH As Double: txtH = labelH

    Dim pt(0 To 2) As Double
    pt(0) = CDbl(bw) / 2
    pt(1) = CDbl(bh) / 2
    pt(2) = 0

    Dim t As Object
    Set t = blkDef.AddText(lbl, pt, txtH)
    t.StyleName = "Standard"
    t.Alignment = 4: t.TextAlignmentPoint = pt
    If rotated90 Then t.Rotation = 1.5707963265
    Set t = Nothing
End Sub

Private Sub AddSheetLabel(ms As Object, sheetNum As Long, xCenter As Double, y As Double)
    Dim pt(0 To 2) As Double
    pt(0) = xCenter
    pt(1) = y
    pt(2) = 0
    Dim t As Object
    Set t = ms.AddText(ChrW(1051) & ChrW(1080) & ChrW(1089) & ChrW(1090) & " " & sheetNum, pt, 60)
    t.StyleName = "Standard"
    t.Alignment = 4
    t.TextAlignmentPoint = pt
    Set t = Nothing
End Sub

Public Sub SendToAutoCAD()
    Dim ws As Worksheet
    Set ws = ThisWorkbook.Sheets(MAIN_SHEET_INDEX)

    Dim shW      As Long:   shW      = ws.Range(SHEET_W_CELL).Value
    Dim shH      As Long:   shH      = ws.Range(SHEET_H_CELL).Value
    Dim kerf     As Long:   kerf     = ws.Range(KERF_CELL).Value
    Dim margin   As Long:   margin   = ws.Range(MARGIN_CELL).Value
    Dim labelPct As Double: labelPct = 50#
    If ws.Range(ACAD_FONT_SIZE).Value <> "" Then
        labelPct = CDbl(ws.Range(ACAD_FONT_SIZE).Value)
    End If
    If shW = 0 Or shH = 0 Then
        MsgBox "Sheet dimensions not set (H1, I1).", vbExclamation: Exit Sub
    End If

    Dim rRow As Long: rRow = ws.Range(RESULT_CELL).Row + 1
    Dim rCol As Long: rCol = ws.Range(RESULT_CELL).Column
    If ws.Cells(rRow, rCol).Value = "" Then
        MsgBox "No placements. Run solver first.", vbExclamation: Exit Sub
    End If

    Dim acad As Object, doc As Object
    Set acad = GetAcadInstance()
    If acad Is Nothing Then
        MsgBox "AutoCAD not running. Open a drawing first.", vbCritical: Exit Sub
    End If
    Set doc = acad.ActiveDocument
    acad.Visible = True

    ' Clear all entities from model space (AOM - synchronous, no timing issues)
    Dim ms As Object: Set ms = doc.ModelSpace
    Dim i As Long
    For i = ms.Count - 1 To 0 Step -1
        ms.Item(i).Delete
    Next i

    ' Remove old cp* block definitions so names can be reused
    Dim blkNames() As String
    Dim nOld As Long: nOld = 0
    Dim blk As Object
    For Each blk In doc.Blocks
        If Left(blk.Name, 2) = "cp" Then
            ReDim Preserve blkNames(nOld)
            blkNames(nOld) = blk.Name
            nOld = nOld + 1
        End If
    Next blk
    For i = 0 To nOld - 1
        On Error Resume Next
        doc.Blocks(blkNames(i)).Delete
        On Error GoTo 0
    Next i

    ' Build one block per placement and insert it via AOM
    Dim r As Long: Dim pIdx As Long: pIdx = 0
    For r = rRow To 10000
        If ws.Cells(r, rCol).Value = "" And ws.Cells(r, rCol + 1).Value = "" Then Exit For

        Dim shIdx As Long: shIdx = CLng(ws.Cells(r, rCol).Value) - 1   ' table is 1-based; convert to 0-based
        Dim px As Long: px = CLng(ws.Cells(r, rCol + 2).Value)          ' X from sheet left (Y-down coords)
        Dim py As Long: py = CLng(ws.Cells(r, rCol + 3).Value)          ' Y from sheet top  (Y-down coords)
        Dim pw As Long: pw = CLng(ws.Cells(r, rCol + 4).Value)          ' placed width
        Dim ph As Long: ph = CLng(ws.Cells(r, rCol + 5).Value)          ' placed height
        ' Rotated layout: solver-X (width) maps to AutoCAD-Y, solver-Y (height) maps to AutoCAD-X.
        ' Sheet occupies [xOff, xOff+shH] x [0, shW] in drawing space.
        Dim xOff As Long: xOff = shIdx * (shH + SHEET_GAP_ACAD)        ' sheet left edge in drawing

        ' Create block definition (origin at 0,0)
        Dim bName As String: bName = "cp" & pIdx
        Dim basePt(0 To 2) As Double  ' (0,0,0)
        Dim blkDef As Object
        Set blkDef = doc.Blocks.Add(basePt, bName)

        ' Block space: (ph+kerf) wide (AutoCAD X = solver Y dir), (pw+kerf) tall (AutoCAD Y = solver X dir)
        ' Expanding by kerf makes adjacent blocks snap flush — no manual kerf offset needed.
        Dim bw As Long: bw = ph + kerf  ' block width  in AutoCAD X
        Dim bh As Long: bh = pw + kerf  ' block height in AutoCAD Y
        Dim rPts(0 To 7) As Double
        rPts(0) = 0:  rPts(1) = 0
        rPts(2) = bw: rPts(3) = 0
        rPts(4) = bw: rPts(5) = bh
        rPts(6) = 0:  rPts(7) = bh
        Dim rectObj As Object
        Set rectObj = blkDef.AddLightWeightPolyline(rPts)
        rectObj.Closed = True

        AddPieceLabel blkDef, bw, bh, pw, ph, CStr(ws.Cells(r, rCol + 1).Value), labelPct

        ' Insert point = bottom-left of block in drawing space
        ' acad_x = xOff + py,  acad_y = shW - px - pw
        Dim insPt(0 To 2) As Double
        insPt(0) = xOff + py
        insPt(1) = shW - px - pw
        insPt(2) = 0
        ms.InsertBlock insPt, bName, 1, 1, 1, 0

        Set rectObj = Nothing: Set blkDef = Nothing
        pIdx = pIdx + 1
    Next r

    ' Draw sheet outlines
    Dim nSheets As Long: nSheets = CLng(ws.Range(OUT_SHEETS_CELL).Value)
    If nSheets < 1 Then nSheets = 1
    Dim si As Long
    For si = 0 To nSheets - 1
        Dim sxOff As Long: sxOff = si * (shH + SHEET_GAP_ACAD)
        ' Inner rectangle — working area after margin
        Dim iPts(0 To 7) As Double
        iPts(0) = sxOff + margin:                iPts(1) = margin
        iPts(2) = sxOff + shH + kerf - margin:   iPts(3) = margin
        iPts(4) = sxOff + shH + kerf - margin:   iPts(5) = shW + kerf - margin
        iPts(6) = sxOff + margin:                iPts(7) = shW + kerf - margin
        Dim innerRect As Object
        Set innerRect = ms.AddLightWeightPolyline(iPts)
        innerRect.Closed = True
        AddSheetLabel ms, si + 1, CDbl(sxOff) + CDbl(shH + kerf) / 2, shW + kerf + 70
    Next si

    acad.ZoomExtents
End Sub

'' == One-time sheet setup =====================================================

' Runs one-time setup: "Can rotate?" checkboxes + algorithm dropdown.
Public Sub SetupAll()
    CreateCheckboxes
    SetupAlgorithmValidation
End Sub

Private Sub SetupAlgorithmValidation()
    Dim ws As Worksheet
    Set ws = ThisWorkbook.Sheets(MAIN_SHEET_INDEX)
    With ws.Range(CFG_ALGORITHM_CELL).Validation
        .Delete
        .Add Type:=xlValidateList, AlertStyle:=xlValidAlertStop, _
             Formula1:="glas - Генетический алгоритм,bfdh - Лучший подходящий,jylanki - Эвристика Джиланки"
    End With
End Sub

' Creates only the header "all" checkbox (cbMain); dc+4 cells are plain
' TRUE/FALSE values now. Kept the cleanup loop below for migration: it
' deletes every checkbox except CFG_RANDOM_SEED_CHK, so re-running this also
' puts an older workbook (with one cbRow<i> per row) into the correct state.
Sub CreateCheckboxes()
    Dim ws As Worksheet
    Set ws = ThisWorkbook.Sheets(MAIN_SHEET_INDEX)

    Dim shp As Object
    For Each shp In ws.CheckBoxes
        If shp.Name <> CFG_RANDOM_SEED_CHK Then shp.Delete
    Next shp

    Dim cbCol     As Long:   cbCol     = DataCol(ws) + 4
    Dim colWidth  As Double: colWidth  = ws.Columns(cbCol).Width
    Dim cell      As Range
    Dim cb        As CheckBox
    Dim mg        As Double: mg = 1#

    Set cell = ws.Cells(DataHeaderRow(ws), cbCol)
    Set cb = ws.CheckBoxes.Add(cell.Left + mg, cell.Top + mg, colWidth - 2*mg, cell.Height - 2*mg)
    cb.Caption  = ""
    cb.OnAction = "Cut.MainCheckboxClick"
    cb.Name     = "cbMain"
End Sub

Sub MainCheckboxClick()
    Dim ws As Worksheet
    Dim mainVal As Boolean
    Dim i As Long

    Set ws = ActiveSheet
    mainVal = (ws.CheckBoxes("cbMain").Value = xlOn)

    Dim dc As Long: dc = DataCol(ws)
    Dim lastRow As Long: lastRow = LastPieceRow(ws)
    For i = DataStartRow(ws) To lastRow
        If Not IsBlankPieceRow(ws, i, dc) Then ws.Cells(i, dc + 4).Value = mainVal
    Next i
End Sub
