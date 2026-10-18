// step2glb: converts a vendor STEP file into a GLB for LabSetup's part library.
//
// Runs as a sidecar process (see docs/DESIGN.md §5), so a crash on a bad STEP file cannot
// take the app down. The app passes two paths, waits with a timeout and reads the JSON
// summary from stdout. Errors go to stderr as one readable line, with a non-zero exit code.
//
// Output conventions match lab-core, not glTF: millimetres, +Z up (+Y forward, +X right).
// STEP files do not say which axis is up; `--source-up y` turns a Y-up model (SolidWorks,
// e.g. every Thorlabs file tried so far) upright. `--standard-gltf` writes glTF's own
// +Y up / metres instead, for viewing in other tools.

// Before OpenCascade: some of its headers include a trimmed-down <windows.h>.
#include <windows.h>

#include <BRepMesh_IncrementalMesh.hxx>
#include <BRep_Builder.hxx>
#include <BRep_Tool.hxx>
#include <Bnd_Box.hxx>
#include <IMeshTools_Parameters.hxx>
#include <Message.hxx>
#include <Message_Messenger.hxx>
#include <Message_PrinterOStream.hxx>
#include <Message_ProgressRange.hxx>
#include <OSD.hxx>
#include <Poly_Triangulation.hxx>
#include <RWGltf_CafWriter.hxx>
#include <RWMesh_CoordinateSystem.hxx>
#include <STEPCAFControl_Reader.hxx>
#include <STEPControl_Reader.hxx>
#include <Standard_Failure.hxx>
#include <Standard_Version.hxx>
#include <TColStd_IndexedDataMapOfStringString.hxx>
#include <TColStd_SequenceOfAsciiString.hxx>
#include <TDF_LabelSequence.hxx>
#include <TDocStd_Document.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Compound.hxx>
#include <XCAFApp_Application.hxx>
#include <XCAFDoc_DocumentTool.hxx>
#include <XCAFDoc_ShapeTool.hxx>
#include <XCAFPrs_DocumentExplorer.hxx>

#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cwchar>
#include <exception>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <unordered_set>

namespace fs = std::filesystem;

namespace {

constexpr const char* kVersion = "0.1.0";

// Exit codes. The app only needs "zero or not", but distinct codes make logs easier to read.
enum ExitCode : int {
  kOk          = 0,
  kUsage       = 1, // bad command line
  kBadInput    = 2, // input missing, unreadable or not a STEP file
  kNoGeometry  = 3, // STEP file read, but it has no surfaces to mesh
  kMeshFailed  = 4, // meshing produced no triangles
  kWriteFailed = 5, // GLB could not be written
  kInternal    = 6, // OpenCascade exception or crash
};

struct Error {
  int         code;
  std::string message;
};

struct Options {
  fs::path input;
  fs::path output;
  double   linearDeflection  = 0.05; // mm
  double   angularDeflection = 0.3;  // rad
  bool     sourceYUp         = false;
  bool     standardGltf      = false;
  bool     verbose           = false;
};

const char* kUsageText =
  "Usage: step2glb <input.step> <output.glb> [options]\n"
  "\n"
  "Converts a STEP file into a binary glTF (GLB) for LabSetup, keeping part names and\n"
  "colours. Output is in millimetres with +Z up (LabSetup's document space).\n"
  "\n"
  "Options:\n"
  "  -l, --linear-deflection <mm>    max distance from mesh to surface (default 0.05)\n"
  "  -a, --angular-deflection <rad>  max angle between adjacent facets (default 0.3)\n"
  "      --source-up <y|z>           up axis of the STEP model (default z); y rotates it to\n"
  "                                  +Z up (SolidWorks exports, e.g. Thorlabs, are y)\n"
  "      --standard-gltf             write glTF's +Y up, metres instead (for other viewers)\n"
  "  -v, --verbose                   print OpenCascade's progress messages to stderr\n"
  "  -h, --help                      show this help\n"
  "      --version                   show version\n"
  "\n"
  "On success prints a one-line JSON summary to stdout and exits 0. On failure prints\n"
  "\"step2glb: error: ...\" to stderr and exits with 1 (usage), 2 (bad input), 3 (no\n"
  "geometry), 4 (meshing failed), 5 (write failed) or 6 (internal error).\n";

std::string utf8(const std::wstring& w) {
  if (w.empty()) return {};
  const int n = WideCharToMultiByte(CP_UTF8, 0, w.data(), (int)w.size(), nullptr, 0, nullptr, nullptr);
  std::string s(n, '\0');
  WideCharToMultiByte(CP_UTF8, 0, w.data(), (int)w.size(), s.data(), n, nullptr, nullptr);
  return s;
}

// OpenCascade takes UTF-8 paths and converts them to wide strings internally.
std::string occtPath(const fs::path& p) { return utf8(p.wstring()); }

std::string jsonEscape(const std::string& s) {
  std::string out;
  for (const char c : s) {
    switch (c) {
      case '"':  out += "\\\""; break;
      case '\\': out += "\\\\"; break;
      case '\n': out += "\\n"; break;
      case '\r': out += "\\r"; break;
      case '\t': out += "\\t"; break;
      default:
        if ((unsigned char)c < 0x20) {
          char buf[8];
          std::snprintf(buf, sizeof buf, "\\u%04x", c);
          out += buf;
        } else {
          out += c;
        }
    }
  }
  return out;
}

double parsePositive(const std::wstring& flag, const wchar_t* value) {
  if (value == nullptr) throw Error{kUsage, utf8(flag) + " needs a value"};
  wchar_t* end = nullptr;
  const double v = std::wcstod(value, &end);
  if (end == value || *end != L'\0' || !std::isfinite(v) || v <= 0.0)
    throw Error{kUsage, utf8(flag) + " must be a positive number, got \"" + utf8(value) + "\""};
  return v;
}

Options parseArgs(int argc, wchar_t** argv) {
  Options opt;
  int     positional = 0;
  for (int i = 1; i < argc; ++i) {
    const std::wstring a    = argv[i];
    const wchar_t*     next = i + 1 < argc ? argv[i + 1] : nullptr;
    if (a == L"-h" || a == L"--help") {
      std::fputs(kUsageText, stdout);
      std::exit(kOk);
    } else if (a == L"--version") {
      std::printf("step2glb %s (OpenCascade %s)\n", kVersion, OCC_VERSION_COMPLETE);
      std::exit(kOk);
    } else if (a == L"-l" || a == L"--linear-deflection") {
      opt.linearDeflection = parsePositive(a, next);
      ++i;
    } else if (a == L"-a" || a == L"--angular-deflection") {
      opt.angularDeflection = parsePositive(a, next);
      ++i;
    } else if (a == L"--source-up") {
      const std::wstring axis = next ? next : L"";
      if (axis != L"y" && axis != L"z") throw Error{kUsage, "--source-up must be y or z"};
      opt.sourceYUp = axis == L"y";
      ++i;
    } else if (a == L"--standard-gltf") {
      opt.standardGltf = true;
    } else if (a == L"-v" || a == L"--verbose") {
      opt.verbose = true;
    } else if (a.size() > 1 && a[0] == L'-') {
      throw Error{kUsage, "unknown option " + utf8(a) + " (see --help)"};
    } else if (positional == 0) {
      opt.input = a;
      ++positional;
    } else if (positional == 1) {
      opt.output = a;
      ++positional;
    } else {
      throw Error{kUsage, "unexpected argument " + utf8(a) + " (see --help)"};
    }
  }
  if (positional < 2) throw Error{kUsage, "expected <input.step> <output.glb> (see --help)"};
  return opt;
}

// Cheap checks with clear messages before OpenCascade gets the file: OpenCascade's own
// diagnostics for a non-STEP file are cryptic.
void checkInput(const fs::path& path) {
  std::error_code ec;
  const auto status = fs::status(path, ec);
  if (ec || !fs::exists(status)) throw Error{kBadInput, "input file not found: " + occtPath(path)};
  if (!fs::is_regular_file(status)) throw Error{kBadInput, "input is not a file: " + occtPath(path)};
  if (fs::file_size(path, ec) == 0) throw Error{kBadInput, "input file is empty: " + occtPath(path)};

  std::ifstream in(path, std::ios::binary);
  if (!in) throw Error{kBadInput, "cannot open input file: " + occtPath(path)};
  char head[1024] = {};
  in.read(head, sizeof head);
  const std::string start(head, (size_t)in.gcount());
  if (start.rfind("PK\x03\x04", 0) == 0)
    throw Error{kBadInput, "input is a ZIP archive; extract the .step file from it first"};
  if (start.rfind("\x1f\x8b", 0) == 0)
    throw Error{kBadInput, "input is gzip-compressed (.stpZ); decompress it first"};
  if (start.find("ISO-10303-21") == std::string::npos)
    throw Error{kBadInput, "input is not a STEP file (no ISO-10303-21 header)"};
}

const char* describe(IFSelect_ReturnStatus status) {
  switch (status) {
    case IFSelect_RetVoid:  return "the file contains no STEP data";
    case IFSelect_RetError: return "the STEP file has syntax errors";
    case IFSelect_RetFail:  return "the STEP file could not be parsed; it may be damaged or truncated";
    case IFSelect_RetStop:  return "reading the STEP file was aborted";
    default:                return "unknown STEP reader status";
  }
}

using Clock = std::chrono::steady_clock;

double secondsSince(Clock::time_point t0) {
  return std::chrono::duration<double>(Clock::now() - t0).count();
}

int run(const Options& opt) {
  const auto t0 = Clock::now();
  checkInput(opt.input);

  const fs::path outDir = fs::absolute(opt.output).parent_path();
  if (!fs::is_directory(outDir)) throw Error{kWriteFailed, "output folder does not exist: " + occtPath(outDir)};

  // Read. The document unit is set before transfer, so the reader converts whatever unit
  // the file uses (the Thorlabs files tried so far are in inches) into millimetres.
  Handle(TDocStd_Document) doc;
  XCAFApp_Application::GetApplication()->NewDocument("BinXCAF", doc);
  XCAFDoc_DocumentTool::SetLengthUnit(doc, 1.0, UnitsMethods_LengthUnit_Millimeter);

  STEPCAFControl_Reader reader;
  reader.SetColorMode(true);
  reader.SetNameMode(true);
  reader.SetLayerMode(false);
  reader.SetPropsMode(false);
  reader.SetGDTMode(false);
  reader.SetMatMode(false);
  reader.SetViewMode(false);

  const IFSelect_ReturnStatus status = reader.ReadFile(occtPath(opt.input).c_str());
  if (status != IFSelect_RetDone) throw Error{kBadInput, describe(status)};
  const double readSeconds = secondsSince(t0);

  TColStd_SequenceOfAsciiString lengthUnits, angleUnits, solidAngleUnits;
  reader.ChangeReader().FileUnits(lengthUnits, angleUnits, solidAngleUnits);
  const std::string sourceUnit = lengthUnits.IsEmpty() ? "unknown" : lengthUnits.First().ToCString();

  const auto t1 = Clock::now();
  if (!reader.Transfer(doc)) throw Error{kNoGeometry, "the STEP file contains no transferable shapes"};

  const Handle(XCAFDoc_ShapeTool) shapeTool = XCAFDoc_DocumentTool::ShapeTool(doc->Main());
  TDF_LabelSequence roots;
  shapeTool->GetFreeShapes(roots);
  if (roots.IsEmpty()) throw Error{kNoGeometry, "the STEP file contains no shapes"};

  TopoDS_Compound all;
  BRep_Builder    builder;
  builder.MakeCompound(all);
  for (const TDF_Label& label : roots) builder.Add(all, XCAFDoc_ShapeTool::GetShape(label));

  TDF_LabelSequence shapes;
  shapeTool->GetShapes(shapes);
  size_t parts = 0;
  for (const TDF_Label& label : shapes)
    if (!XCAFDoc_ShapeTool::IsAssembly(label)) ++parts;
  size_t instances = 0;
  for (XCAFPrs_DocumentExplorer it(doc, XCAFPrs_DocumentExplorerFlags_OnlyLeafNodes); it.More(); it.Next())
    ++instances;
  const double transferSeconds = secondsSince(t1);

  // Mesh. Instances share their TShape, so each part is triangulated once.
  const auto            t2 = Clock::now();
  IMeshTools_Parameters mesh;
  mesh.Deflection = opt.linearDeflection;
  mesh.Angle      = opt.angularDeflection;
  mesh.Relative   = false;
  mesh.InParallel = true;
  BRepMesh_IncrementalMesh mesher(all, mesh);

  // Count what the GLB will hold, and take the bounds from the mesh nodes themselves
  // (exactly the GLB's extents; B-rep bounds include tolerances and meshing slack).
  size_t faces = 0, unmeshedFaces = 0, triangles = 0, uniqueTriangles = 0;
  std::unordered_set<const Poly_Triangulation*> seen;
  Bnd_Box box;
  for (TopExp_Explorer ex(all, TopAbs_FACE); ex.More(); ex.Next()) {
    ++faces;
    TopLoc_Location                   loc;
    const Handle(Poly_Triangulation)& tri = BRep_Tool::Triangulation(TopoDS::Face(ex.Current()), loc);
    if (tri.IsNull() || tri->NbTriangles() == 0) {
      ++unmeshedFaces;
      continue;
    }
    triangles += tri->NbTriangles();
    if (seen.insert(tri.get()).second) uniqueTriangles += tri->NbTriangles();
    const gp_Trsf trsf = loc.Transformation();
    for (Standard_Integer i = 1; i <= tri->NbNodes(); ++i) box.Add(tri->Node(i).Transformed(trsf));
  }
  if (faces == 0) throw Error{kNoGeometry, "the STEP file has no surfaces (wireframe or points only)"};
  if (triangles == 0) throw Error{kMeshFailed, "meshing produced no triangles"};
  if (unmeshedFaces > 0)
    std::fprintf(stderr, "step2glb: warning: %zu of %zu faces could not be meshed and are missing\n", unmeshedFaces, faces);

  double xmin, ymin, zmin, xmax, ymax, zmax;
  box.Get(xmin, ymin, zmin, xmax, ymax, zmax);
  if (opt.sourceYUp) { // (x, y, z) -> (x, -z, y), as the converter below does
    const double y0 = ymin, y1 = ymax;
    ymin = -zmax;
    ymax = -zmin;
    zmin = y0;
    zmax = y1;
  }
  const double meshSeconds = secondsSince(t2);

  // Write to a temporary name first, so a failed or killed run never leaves a partial GLB.
  const auto t3      = Clock::now();
  fs::path   partial = opt.output;
  partial += ".partial";
  {
    RWGltf_CafWriter                  writer(occtPath(partial).c_str(), true);
    RWMesh_CoordinateSystemConverter& cs = writer.ChangeCoordinateSystemConverter();
    cs.SetInputLengthUnit(0.001);
    cs.SetInputCoordinateSystem(opt.sourceYUp ? RWMesh_CoordinateSystem_Yup : RWMesh_CoordinateSystem_Zup);
    if (opt.standardGltf) {
      cs.SetOutputLengthUnit(1.0);
      cs.SetOutputCoordinateSystem(RWMesh_CoordinateSystem_glTF);
    } else {
      cs.SetOutputLengthUnit(0.001);
      cs.SetOutputCoordinateSystem(RWMesh_CoordinateSystem_Zup);
    }
    writer.SetTransformationFormat(RWGltf_WriterTrsfFormat_Compact);
    writer.SetMergeFaces(true); // one primitive per part and colour, not one per B-rep face
    writer.SetParallel(true);

    // Stored in asset.extras, so the importer can check the conventions it gets.
    TColStd_IndexedDataMapOfStringString info;
    info.Add("labsetup_units", opt.standardGltf ? "m" : "mm");
    info.Add("labsetup_up_axis", opt.standardGltf ? "+Y" : "+Z");
    info.Add("labsetup_source_file", occtPath(opt.input.filename()).c_str());
    info.Add("labsetup_source_up_axis", opt.sourceYUp ? "+Y" : "+Z");
    info.Add("labsetup_linear_deflection_mm", TCollection_AsciiString(opt.linearDeflection));
    info.Add("labsetup_angular_deflection_rad", TCollection_AsciiString(opt.angularDeflection));
    info.Add("labsetup_step2glb_version", kVersion);

    if (!writer.Perform(doc, info, Message_ProgressRange())) {
      std::error_code ec;
      fs::remove(partial, ec);
      throw Error{kWriteFailed, "could not write " + occtPath(opt.output)};
    }
  }
  std::error_code ec;
  fs::rename(partial, opt.output, ec);
  if (ec) {
    fs::remove(partial, ec);
    throw Error{kWriteFailed, "could not write " + occtPath(opt.output) + ": " + ec.message()};
  }
  const double writeSeconds = secondsSince(t3);
  const auto   bytes        = fs::file_size(opt.output, ec);

  std::ostringstream json;
  json.setf(std::ios::fixed);
  json.precision(3);
  json << "{\"input\":\"" << jsonEscape(occtPath(opt.input)) << "\""
       << ",\"output\":\"" << jsonEscape(occtPath(opt.output)) << "\""
       << ",\"units\":\"" << (opt.standardGltf ? "m" : "mm") << "\""
       << ",\"up_axis\":\"" << (opt.standardGltf ? "+Y" : "+Z") << "\""
       << ",\"source_length_unit\":\"" << jsonEscape(sourceUnit) << "\""
       << ",\"source_up_axis\":\"" << (opt.sourceYUp ? "+Y" : "+Z") << "\""
       << ",\"parts\":" << parts << ",\"instances\":" << instances
       << ",\"faces\":" << faces << ",\"unmeshed_faces\":" << unmeshedFaces
       << ",\"triangles\":" << triangles << ",\"triangles_unique\":" << uniqueTriangles
       << ",\"bbox_mm\":{\"min\":[" << xmin << "," << ymin << "," << zmin << "],\"max\":[" << xmax << ","
       << ymax << "," << zmax << "]}"
       << ",\"bytes\":" << bytes << ",\"seconds\":{\"read\":" << readSeconds << ",\"transfer\":" << transferSeconds
       << ",\"mesh\":" << meshSeconds << ",\"write\":" << writeSeconds << ",\"total\":" << secondsSince(t0) << "}}";
  std::cout << json.str() << std::endl;
  return kOk;
}

[[noreturn]] void die(int code, const std::string& message) {
  std::fflush(stdout);
  std::fprintf(stderr, "step2glb: error: %s\n", message.c_str());
  std::fflush(stderr);
  std::_Exit(code);
}

// Last resort for crashes the C++ handlers below do not see (e.g. in a meshing worker
// thread): report and exit instead of showing the Windows crash dialog.
LONG WINAPI onUnhandledException(EXCEPTION_POINTERS* info) {
  char buf[96];
  std::snprintf(buf, sizeof buf, "crashed (Windows exception 0x%08lX)",
                info && info->ExceptionRecord ? info->ExceptionRecord->ExceptionCode : 0ul);
  die(kInternal, buf);
}

} // namespace

int wmain(int argc, wchar_t** argv) {
  // No error dialogs: the app runs this headless and would otherwise wait for the timeout.
  SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
  _set_abort_behavior(0, _WRITE_ABORT_MSG | _CALL_REPORTFAULT);
  // Translate access violations etc. into Standard_Failure (needs /EHa); no FP traps.
  OSD::SetSignal(OSD_SignalMode_Set, false);
  SetUnhandledExceptionFilter(onUnhandledException);
  std::set_terminate([] { die(kInternal, "terminated by an unhandled exception"); });

  try {
    const Options opt = parseArgs(argc, argv);

    // OpenCascade reports through the default messenger, which writes to stdout. Keep
    // stdout for the JSON summary and send its messages (failures only, unless -v) to stderr.
    const Handle(Message_Messenger)& messenger = Message::DefaultMessenger();
    messenger->RemovePrinters(STANDARD_TYPE(Message_PrinterOStream));
    messenger->AddPrinter(new Message_PrinterOStream("cerr", false, opt.verbose ? Message_Info : Message_Fail));

    return run(opt);
  } catch (const Error& e) {
    die(e.code, e.message);
  } catch (const Standard_Failure& e) {
    die(kInternal, std::string("OpenCascade ") + e.DynamicType()->Name() + ": " + e.GetMessageString());
  } catch (const std::exception& e) {
    die(kInternal, e.what());
  } catch (...) {
    die(kInternal, "unknown exception");
  }
}
