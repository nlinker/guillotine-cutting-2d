Attribute VB_Name = "Cut"
''
'' Cut.bas  -  VBA module: launches cut.exe and reads progress via named pipe
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

Private Const SHEET_NAME       As String = "Sheet1"
Private Const PIPE_NAME        As String = "\\.\pipe\cut_progress"
Private Const GENERIC_READ     As Long   = &H80000000
Private Const OPEN_EXISTING    As Long   = 3
Private Const FILE_ATTR_NORMAL As Long   = &H80
Private Const BUFFER_SIZE      As Long   = 8192
Private Const MAX_PIECE_ROWS   As Long   = 100   ' max rows in the piece table

Private Const SHEET_W_CELL     As String = "J1"  ' sheet width (mm)
Private Const SHEET_H_CELL     As String = "K1"  ' sheet height (mm)
Private Const KERF_CELL        As String = "K2"  ' blade kerf width (mm)
Private Const MARGIN_CELL      As String = "K3"  ' edge margin (mm)
Private Const ACAD_FONT_SIZE   As String = "K4"  ' label text height in drawing units (mm)
Private Const EDGE_MARGIN_CELL As String = "K5"  ' edging overhang per strip (mm); default 40
Private Const DATA_CELL        As String = "A5"  ' top-left of piece table ("Panel" label column, first input row)
Private Const RESULT_CELL      As String = "O8"  ' top-left of placement table ("Sheet" label column, first result row)

Private Const CFG_RANDOM_SEED_CHK As String = "ChkRandomSeed"  ' checkbox: randomize seed on each run

Private Const CFG_SEED_CELL      As String = "N1"  ' base random seed (--seed)
Private Const CFG_GENS_CELL      As String = "N2"  ' generations per run (--gens)
Private Const CFG_POP_CELL       As String = "N3"  ' population size (--pop)
Private Const CFG_ALGORITHM_CELL      As String = "N4"  ' decoder algorithm: glas or slas
Private Const CFG_LARGE_AREA_CELL     As String = "N5"  ' large_area_threshold (0 = auto)
Private Const CFG_LONG_DIM_CELL       As String = "N6"  ' long_dim_threshold   (0 = auto)
Private Const OUT_STATUS_CELL  As String = "R1"  ' status text
Private Const OUT_GEN_CELL     As String = "R2"  ' current generation
Private Const OUT_OBJ_CELL     As String = "R3"  ' best objective
Private Const OUT_SHEETS_CELL  As String = "R4"  ' sheets used
Private Const OUT_CUT_CELL     As String = "R5"  ' cuts used for each of the sheets

Private Const CANVAS_RANGE     As String = "I8:N8"  ' top row of canvas; left col = draw origin, right col = width boundary
Private Const CANVAS_SHEET_GAP As Double = 14#   ' gap between sheets in points
Private Const SHEET_GAP_ACAD   As Long   = 150   ' gap between sheets exported to AutoCAD (drawing units)
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
        idx = pl("piespec_idx") + 1
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
    Dim canvasW    As Double: canvasW     = cvs.Cells(1, cvs.Columns.Count).Left - originLeft

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
        Dim idx   As Long: idx   = pl("piespec_idx") + 1
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
    Dim sPieces As String
    Dim bFirst  As Boolean: bFirst = True
    Dim i As Long: i = DataStartRow(ws)

    Do
        Dim w As Long, h As Long
        w = 0: h = 0
        If ws.Cells(i, dc + 1).Value <> "" Then w = CLng(ws.Cells(i, dc + 1).Value)
        If ws.Cells(i, dc + 2).Value <> "" Then h = CLng(ws.Cells(i, dc + 2).Value)
        If w = 0 Or h = 0 Then Exit Do

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
        i = i + 1
    Loop

    BuildProblemJson = "{""sheet"":{""width"":" & CStr(sheetWidth) & _
                       ",""height"":" & CStr(sheetHeight) & "}" & _
                       ",""kerf"":" & CStr(kerf) & _
                       ",""margin"":" & CStr(margin) & _
                       ",""piespecs"":[" & sPieces & "]}"
End Function

'' == Main macro ===============================================================

Public Sub RunCut()
    If g_Running Then
        MsgBox "Solver is already running!", vbInformation
        Exit Sub
    End If

    Dim ws As Worksheet
    Set ws = ThisWorkbook.sheets(SHEET_NAME)

    Dim exePath As String
    exePath = Trim(ws.Cells(1, 2).Value)  ' B1
    If Dir(exePath) = "" Then
        MsgBox "cut.exe not found: " & exePath & Chr(13) & _
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
    Dim nLargeArea As Long: nLargeArea = 0
    If ws.Range(CFG_LARGE_AREA_CELL).Value <> "" Then nLargeArea = CLng(ws.Range(CFG_LARGE_AREA_CELL).Value)
    Dim nLongDim As Long: nLongDim = 0
    If ws.Range(CFG_LONG_DIM_CELL).Value <> "" Then nLongDim = CLng(ws.Range(CFG_LONG_DIM_CELL).Value)

    ' Launch cut.exe (non-blocking Shell); threads = 0 means auto-detect
    Dim cmd As String
    cmd = Chr(34) & exePath & Chr(34) & " calc --json " & Chr(34) & tmpFile & Chr(34) _
        & " --seed " & nSeed & " --gens " & nGens & " --pop " & nPop _
        & " --algorithm " & sAlgorithm
    If nLargeArea > 0 Then cmd = cmd & " --large-area-threshold " & nLargeArea
    If nLongDim > 0 Then cmd = cmd & " --long-dim-threshold " & nLongDim
    Shell cmd, vbHide

    ' Give cut.exe time to create the pipe
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
               "Make sure cut.exe started successfully.", vbCritical
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

' Calculates total edge banding length (mm) from edging columns F and G.
' F = number of width edges (0-2), G = number of height edges (0-2).
' Overhang (припуск) is read from EDGE_MARGIN_CELL (K5); default = 40 mm if empty.
' Usage: put =TotalEdgeLength() in any cell; recalculates on every sheet change.
Public Function TotalEdgeLength() As Long
    Application.Volatile
    Dim ws As Worksheet
    Set ws = ThisWorkbook.Sheets(SHEET_NAME)

    Dim overhang As Long
    overhang = CLng(ws.Range(EDGE_MARGIN_CELL).Value)

    Dim dc As Long:    dc    = DataCol(ws)
    Dim total As Long: total = 0
    Dim i As Long

    For i = DataStartRow(ws) To DataEndRow(ws)
        Dim w As Long, h As Long
        w = 0: h = 0
        If ws.Cells(i, dc + 1).Value <> "" Then w = CLng(ws.Cells(i, dc + 1).Value)
        If ws.Cells(i, dc + 2).Value <> "" Then h = CLng(ws.Cells(i, dc + 2).Value)
        If w = 0 Or h = 0 Then Exit For

        Dim cnt As Long: cnt = 0
        If ws.Cells(i, dc + 3).Value <> "" Then cnt = CLng(ws.Cells(i, dc + 3).Value)

        Dim ew As Long: ew = 0
        Dim eh As Long: eh = 0
        If ws.Cells(i, dc + 5).Value <> "" Then ew = CLng(ws.Cells(i, dc + 5).Value)
        If ws.Cells(i, dc + 6).Value <> "" Then eh = CLng(ws.Cells(i, dc + 6).Value)

        total = total + cnt * ew * (w + overhang)
        total = total + cnt * eh * (h + overhang)
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
    Set ws = ThisWorkbook.sheets(SHEET_NAME)
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
    Set ws = ThisWorkbook.sheets(SHEET_NAME)

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
    Set ws = ThisWorkbook.sheets(SHEET_NAME)
    With ws.Range(CFG_ALGORITHM_CELL).Validation
        .Delete
        .Add Type:=xlValidateList, AlertStyle:=xlValidAlertStop, _
             Formula1:="glas,slas,bfdh,gbaf,simple"
    End With
End Sub

'' == Checkboxes for "Can rotate?" =============================================

Sub CreateCheckboxes()
    Dim ws As Worksheet
    Set ws = ThisWorkbook.sheets(SHEET_NAME)

    Dim shp As Object
    For Each shp In ws.CheckBoxes
        If shp.Name <> CFG_RANDOM_SEED_CHK Then shp.Delete
    Next shp

    Dim cbCol     As Long:   cbCol     = DataCol(ws) + 4
    Dim colWidth  As Double: colWidth  = ws.Columns(cbCol).Width
    Dim cell      As Range
    Dim cb        As CheckBox
    Dim i         As Integer

    Dim mg As Double: mg = 1#

    Set cell = ws.Cells(DataHeaderRow(ws), cbCol)
    Set cb = ws.CheckBoxes.Add(cell.Left + mg, cell.Top + mg, colWidth - 2*mg, cell.Height - 2*mg)
    cb.Caption  = "all"
    cb.OnAction = "Cut.MainCheckboxClick"
    cb.Name     = "cbMain"

    For i = DataStartRow(ws) To DataEndRow(ws)
        Set cell = ws.Cells(i, cbCol)
        Set cb = ws.CheckBoxes.Add(cell.Left + mg, cell.Top + mg, colWidth - 2*mg, cell.Height - 2*mg)
        cb.LinkedCell = ws.Cells(i, DataCol(ws) + 4).Address
        cb.Caption    = ""
        cb.Name       = "cbRow" & i
    Next i
End Sub

Sub MainCheckboxClick()
    Dim ws As Worksheet
    Dim mainVal As Boolean
    Dim i As Integer

    Set ws = ActiveSheet
    mainVal = (ws.CheckBoxes("cbMain").Value = xlOn)

    Dim dc As Long: dc = DataCol(ws)
    For i = DataStartRow(ws) To DataEndRow(ws)
        If ws.Cells(i, dc + 4).Value <> "" Then
            ws.Cells(i, dc + 4).Value = mainVal
        End If
    Next i
End Sub
